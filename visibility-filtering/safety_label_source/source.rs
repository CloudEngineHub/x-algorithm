use crate::models::tweet_timestamp_ms;
use quanta::Clock;
use rustc_hash::FxHashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;
use xai_visibility_filtering_proto as vf_pb;

use super::expiring_cache::{ExpiringCache, Lookup};
use super::lookup::{LookupError, LookupResults, RemoteSource};
use super::metrics::{self, BatchStage, CacheResult, CacheTier};

const TTL: Duration = Duration::from_secs(10);

fn is_future_dated(tweet_id: u64, now: SystemTime) -> bool {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(tweet_timestamp_ms(tweet_id)))
        .is_none_or(|created_at| created_at > now)
}

fn system_unix_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn label_is_live(expires_at_msec: Option<i64>, now_ms: i64) -> bool {
    match expires_at_msec {
        Some(expires_at_msec) => expires_at_msec.wrapping_sub(now_ms) > 0,
        None => true,
    }
}

fn drop_expired_labels(map: &vf_pb::SafetyLabelMap, now_ms: i64) -> vf_pb::SafetyLabelMap {
    vf_pb::SafetyLabelMap {
        labels: map
            .labels
            .iter()
            .filter(|(_, label)| label_is_live(label.expires_at_msec, now_ms))
            .map(|(id, label)| (*id, label.clone()))
            .collect(),
    }
}

pub struct SafetyLabelSource {
        cache: Option<ExpiringCache<u64, Arc<vf_pb::SafetyLabelMap>>>,
    remote: Arc<RemoteSource>,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl SafetyLabelSource {
    pub(crate) fn new(remote: Arc<RemoteSource>, cache_capacity: Option<usize>) -> Self {
        Self::with_clock(remote, cache_capacity, Clock::new())
    }

    fn with_clock(remote: Arc<RemoteSource>, cache_capacity: Option<usize>, clock: Clock) -> Self {
        Self::with_clocks(remote, cache_capacity, clock, Arc::new(system_unix_ms))
    }

    fn with_clocks(
        remote: Arc<RemoteSource>,
        cache_capacity: Option<usize>,
        clock: Clock,
        now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        info!(
            cache_capacity = cache_capacity.unwrap_or(0),
            ttl_secs = TTL.as_secs(),
            "Local safety-label cache initialized (single pool, per-entry TTL)"
        );
        Self {
            cache: cache_capacity.map(|capacity| ExpiringCache::with_clock(capacity, clock)),
            remote,
            now_ms,
        }
    }

    pub async fn get(
        &self,
        ids: &[u64],
    ) -> FxHashMap<u64, Result<Arc<vf_pb::SafetyLabelMap>, LookupError>> {
        let total = ids.len();
        let mut results: FxHashMap<u64, Result<Arc<vf_pb::SafetyLabelMap>, LookupError>> =
            FxHashMap::with_capacity_and_hasher(total, Default::default());

        let (local_misses, expired) = self.get_local(ids, &mut results);
        let batch_size = local_misses.len();

        let remote_results = self.remote.get(&local_misses).await;
        self.backfill_local(remote_results, &mut results);

        Self::emit_stats(total - batch_size, batch_size, expired);

        let now_ms = (self.now_ms)();
        let mut dropped = 0;
        for labels in results.values_mut() {
            let Ok(map) = labels else { continue };
            if map
                .labels
                .values()
                .any(|label| !label_is_live(label.expires_at_msec, now_ms))
            {
                let live = drop_expired_labels(map, now_ms);
                dropped += map.labels.len() - live.labels.len();
                *map = Arc::new(live);
            }
        }
        metrics::record_expired_labels(dropped);

        results
    }

    fn get_local(
        &self,
        ids: &[u64],
        results: &mut FxHashMap<u64, Result<Arc<vf_pb::SafetyLabelMap>, LookupError>>,
    ) -> (Vec<u64>, usize) {
        let Some(cache) = &self.cache else {
            return (ids.to_vec(), 0);
        };
        let mut misses = Vec::with_capacity(ids.len());
        let mut expired = 0;
        for &tweet_id in ids {
            match cache.get(&tweet_id) {
                Lookup::Found(labels) => {
                    results.insert(tweet_id, Ok(labels));
                }
                Lookup::Expired => {
                    expired += 1;
                    misses.push(tweet_id);
                }
                Lookup::NotFound => misses.push(tweet_id),
            }
        }
        (misses, expired)
    }

    fn backfill_local(
        &self,
        remote_results: LookupResults,
        results: &mut FxHashMap<u64, Result<Arc<vf_pb::SafetyLabelMap>, LookupError>>,
    ) {
        let wall_now = SystemTime::now();
        for (id, result) in remote_results {
            let result = result.map(Arc::new);
            if let Some(cache) = &self.cache
                && let Ok(label_map) = &result
                && !is_future_dated(id, wall_now)
            {
                cache.insert(id, Arc::clone(label_map), TTL);
            }
            results.insert(id, result);
        }
    }

    fn emit_stats(hits: usize, batch_size: usize, expired: usize) {
        metrics::record_cache_keys(CacheTier::Local, CacheResult::Hit, hits);
        metrics::record_cache_keys(CacheTier::Local, CacheResult::Miss, batch_size - expired);
        metrics::record_cache_keys(CacheTier::Local, CacheResult::Expired, expired);
        metrics::record_batch_size(BatchStage::LocalMiss, batch_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::safety_label_source::cached_value::tests::{
        cached_value_found_mval, cached_value_not_found,
    };
    use crate::safety_label_source::codec::RawSafetyLabel;
    use crate::safety_label_source::lookup::RemoteSource;
    use crate::safety_label_source::lookup::{LookupResults, ManhattanLookup, TwemcacheLookup};
    use crate::safety_label_source::manhattan::ManhattanSource;
    use crate::safety_label_source::mh_client::{FetchResult, ManhattanLabelFetcher};
    use crate::safety_label_source::twemcache::{CacheRead, TwemcacheSource};
    use crate::safety_label_source::types::TwemcacheOutcome;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    use tonic::async_trait;
    use xai_cache::{KVCacheError, Key, Value};

    struct FakeTwemcache {
        results: HashMap<Key, std::result::Result<Option<Value>, KVCacheError>>,
        fetched_keys: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl CacheRead for FakeTwemcache {
        async fn multi_get(
            &self,
            keys: &[Key],
        ) -> HashMap<Key, std::result::Result<Option<Value>, KVCacheError>> {
            self.fetched_keys.fetch_add(keys.len(), Ordering::SeqCst);
            self.results.clone()
        }
    }

    struct FakeLabelFetcher {
        items: HashMap<i64, Vec<RawSafetyLabel>>,
    }

    #[async_trait]
    impl ManhattanLabelFetcher for FakeLabelFetcher {
        async fn fetch_labels(
            &self,
            tweet_ids: &[i64],
        ) -> Result<Vec<FetchResult>, xai_manhattan::ManhattanError> {
            Ok(tweet_ids
                .iter()
                .map(|id| Ok(self.items.get(id).cloned().unwrap_or_default()))
                .collect())
        }
    }

    struct FailingLabelFetcher;

    #[async_trait]
    impl ManhattanLabelFetcher for FailingLabelFetcher {
        async fn fetch_labels(
            &self,
            _: &[i64],
        ) -> Result<Vec<FetchResult>, xai_manhattan::ManhattanError> {
            Err(xai_manhattan::ManhattanError::NativeProtocol(
                "manhattan unavailable".into(),
            ))
        }
    }

    fn make_source_with_clock(
        cache_results: HashMap<Key, std::result::Result<Option<Value>, KVCacheError>>,
        mh_items: HashMap<i64, Vec<RawSafetyLabel>>,
        clock: Clock,
    ) -> SafetyLabelSource {
        make_counting_source_with_clock(cache_results, mh_items, clock).0
    }

    fn make_counting_source(
        cache_results: HashMap<Key, std::result::Result<Option<Value>, KVCacheError>>,
        mh_items: HashMap<i64, Vec<RawSafetyLabel>>,
    ) -> (SafetyLabelSource, Arc<AtomicUsize>) {
        make_counting_source_with_clock(cache_results, mh_items, Clock::new())
    }

    fn make_counting_source_with_clock(
        cache_results: HashMap<Key, std::result::Result<Option<Value>, KVCacheError>>,
        mh_items: HashMap<i64, Vec<RawSafetyLabel>>,
        clock: Clock,
    ) -> (SafetyLabelSource, Arc<AtomicUsize>) {
        make_counting_source_with_fetcher(
            cache_results,
            Arc::new(FakeLabelFetcher { items: mh_items }),
            clock,
            Some(1024),
        )
    }

    fn make_counting_source_with_fetcher(
        cache_results: HashMap<Key, std::result::Result<Option<Value>, KVCacheError>>,
        fetcher: Arc<dyn ManhattanLabelFetcher>,
        clock: Clock,
        l1_capacity: Option<usize>,
    ) -> (SafetyLabelSource, Arc<AtomicUsize>) {
        let remote_keys = Arc::new(AtomicUsize::new(0));
        let twemcache = Arc::new(TwemcacheSource::with_cache(Arc::new(FakeTwemcache {
            results: cache_results,
            fetched_keys: Arc::clone(&remote_keys),
        })));
        let manhattan = Arc::new(ManhattanSource::new(fetcher));
        let remote = Arc::new(RemoteSource::new(twemcache, manhattan));
        (
            SafetyLabelSource::with_clock(remote, l1_capacity, clock),
            remote_keys,
        )
    }

    fn l1_expiry(source: &SafetyLabelSource, tweet_id: u64) -> Option<quanta::Instant> {
        source
            .cache
            .as_ref()
            .and_then(|cache| cache.expiry_of(&tweet_id))
    }

    fn cache_key(tweet_id: u64) -> Key {
        Key::new(format!("slm_{tweet_id}").into_bytes()).unwrap()
    }

    fn tweet_id_created_at(created_at: SystemTime) -> u64 {
        let since_epoch = created_at.duration_since(UNIX_EPOCH).unwrap();
        let created_ms = since_epoch
            .as_secs()
            .checked_mul(1000)
            .and_then(|secs_ms| secs_ms.checked_add(u64::from(since_epoch.subsec_millis())))
            .unwrap();
        created_ms.checked_sub(tweet_timestamp_ms(0)).unwrap() << 22
    }

    fn future_tweet_id() -> u64 {
        tweet_id_created_at(SystemTime::now() + Duration::from_secs(60))
    }

    #[tokio::test]
    async fn get_backfills_remote_successes_into_l1() {
        let (source, remote_keys) = make_counting_source(
            HashMap::from([(cache_key(42), Ok(Some(cached_value_found_mval())))]),
            HashMap::new(),
        );

        let results1 = source.get(&[42]).await;
        let first = Arc::clone(results1.get(&42).unwrap().as_ref().unwrap());
        assert_eq!(remote_keys.load(Ordering::SeqCst), 1);
        assert!(l1_expiry(&source, 42).is_some());

        let results2 = source.get(&[42]).await;
        assert!(Arc::ptr_eq(
            results2.get(&42).unwrap().as_ref().unwrap(),
            &first
        ));
        assert_eq!(remote_keys.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn get_never_backfills_a_remote_error_into_l1() {
        let (source, remote_keys) = make_counting_source_with_fetcher(
            HashMap::new(),
            Arc::new(FailingLabelFetcher),
            Clock::new(),
            Some(1024),
        );

        let results1 = source.get(&[42]).await;
        assert!(results1.get(&42).unwrap().is_err());
        assert!(l1_expiry(&source, 42).is_none());

        let results2 = source.get(&[42]).await;
        assert!(results2.get(&42).unwrap().is_err());
        assert_eq!(remote_keys.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn without_an_l1_every_read_goes_remote() {
        let (source, remote_keys) = make_counting_source_with_fetcher(
            HashMap::from([(cache_key(42), Ok(Some(cached_value_found_mval())))]),
            Arc::new(FakeLabelFetcher {
                items: HashMap::new(),
            }),
            Clock::new(),
            None,
        );

        for _ in 0..2 {
            assert!(source.get(&[42]).await.get(&42).unwrap().is_ok());
        }
        assert_eq!(remote_keys.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn get_does_not_backfill_future_tweet_id_into_l1() {
        let tweet_id = future_tweet_id();
        let source = make_counting_source(
            HashMap::from([(cache_key(tweet_id), Ok(Some(cached_value_found_mval())))]),
            HashMap::new(),
        )
        .0;

        let results = source.get(&[tweet_id]).await;

        assert!(results.get(&tweet_id).unwrap().is_ok());
        assert!(l1_expiry(&source, tweet_id).is_none());
    }

    #[tokio::test]
    async fn get_backfills_negative_cache_into_l1() {
        let (source, remote_keys) = make_counting_source(
            HashMap::from([(cache_key(42), Ok(Some(cached_value_not_found())))]),
            HashMap::new(),
        );

        let results1 = source.get(&[42]).await;
        assert!(
            results1
                .get(&42)
                .unwrap()
                .as_ref()
                .unwrap()
                .labels
                .is_empty()
        );
        assert!(l1_expiry(&source, 42).is_some());

        let results2 = source.get(&[42]).await;
        assert!(
            results2
                .get(&42)
                .unwrap()
                .as_ref()
                .unwrap()
                .labels
                .is_empty()
        );
        assert_eq!(remote_keys.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expired_entry_is_refetched_and_restamped() {
        let (clock, mock) = Clock::mock();
        let tweet_id = tweet_id_created_at(SystemTime::now() - Duration::from_secs(60));
        let source = make_source_with_clock(
            HashMap::from([(cache_key(tweet_id), Ok(Some(cached_value_found_mval())))]),
            HashMap::new(),
            clock,
        );

        source.get(&[tweet_id]).await;
        let first_expiry = l1_expiry(&source, tweet_id).expect("backfilled");

        mock.increment(TTL + Duration::from_secs(1));
        source.get(&[tweet_id]).await;
        let restamped = l1_expiry(&source, tweet_id).expect("re-backfilled");

        assert!(restamped > first_expiry);
    }

    fn proto_label(expires_at_msec: Option<i64>) -> vf_pb::SafetyLabel {
        vf_pb::SafetyLabel {
            expires_at_msec,
            ..vf_pb::SafetyLabel::default()
        }
    }

    struct HitMap {
        map: vf_pb::SafetyLabelMap,
        fetches: AtomicUsize,
    }

    #[async_trait]
    impl TwemcacheLookup for HitMap {
        async fn get(&self, ids: &[u64]) -> FxHashMap<u64, TwemcacheOutcome> {
            self.fetches.fetch_add(ids.len(), Ordering::SeqCst);
            ids.iter()
                .map(|&id| (id, TwemcacheOutcome::Hit(self.map.clone())))
                .collect()
        }
    }

    struct EmptyManhattan;

    #[async_trait]
    impl ManhattanLookup for EmptyManhattan {
        async fn get(&self, _: &[u64]) -> LookupResults {
            FxHashMap::default()
        }
    }

    fn label_ids(map: &vf_pb::SafetyLabelMap) -> Vec<i32> {
        let mut ids: Vec<i32> = map.labels.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    #[tokio::test]
    async fn a_cached_label_drops_out_after_it_expires_without_a_refetch() {
        let now = Arc::new(AtomicI64::new(1_000));
        let now_ms = Arc::clone(&now);
        let hit = Arc::new(HitMap {
            map: vf_pb::SafetyLabelMap {
                labels: HashMap::from([
                    (1, proto_label(None)),
                    (2, proto_label(Some(1_000))),
                    (3, proto_label(Some(5_000))),
                ]),
            },
            fetches: AtomicUsize::new(0),
        });
        let source = SafetyLabelSource::with_clocks(
            Arc::new(RemoteSource::new(
                Arc::clone(&hit),
                Arc::new(EmptyManhattan),
            )),
            Some(8),
            Clock::new(),
            Arc::new(move || now_ms.load(Ordering::Relaxed)),
        );

        let first = source.get(&[42]).await;
        assert_eq!(
            label_ids(first.get(&42).unwrap().as_ref().unwrap()),
            vec![1, 3]
        );
        assert_eq!(hit.fetches.load(Ordering::SeqCst), 1);

        now.store(5_000, Ordering::Relaxed);
        let later = source.get(&[42]).await;
        assert_eq!(
            label_ids(later.get(&42).unwrap().as_ref().unwrap()),
            vec![1]
        );
        assert_eq!(hit.fetches.load(Ordering::SeqCst), 1);

        now.store(0, Ordering::Relaxed);
        let rewound = source.get(&[42]).await;
        assert_eq!(
            label_ids(rewound.get(&42).unwrap().as_ref().unwrap()),
            vec![1, 2, 3]
        );
        assert_eq!(hit.fetches.load(Ordering::SeqCst), 1);
    }
}
