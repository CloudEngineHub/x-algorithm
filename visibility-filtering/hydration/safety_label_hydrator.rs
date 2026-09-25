use crate::models::{SafetyLabelMap, TweetId};
use crate::safety_label_source::lookup::LookupError;
use std::collections::HashMap;
use std::sync::Arc;
use xai_visibility_filtering_proto as vf_pb;

pub struct SafetyLabelHydration {
    pub label_types: HashMap<TweetId, SafetyLabelMap>,
    pub label_response: HashMap<TweetId, Arc<vf_pb::SafetyLabelMap>>,
}

impl SafetyLabelHydration {
    pub(super) fn new(
        tweet_ids: &[TweetId],
        resolved: &HashMap<u64, Result<Arc<vf_pb::SafetyLabelMap>, LookupError>>,
    ) -> Self {
        let mut label_types = HashMap::with_capacity(tweet_ids.len());
        let mut label_response = HashMap::with_capacity(tweet_ids.len());
        for tweet_id in tweet_ids {
            match resolved
                .get(&tweet_id.0)
                .and_then(|result| result.as_ref().ok())
            {
                Some(label_map) => {
                    label_types
                        .insert(*tweet_id, SafetyLabelMap::from_proto_label_types(label_map));
                    label_response.insert(*tweet_id, Arc::clone(label_map));
                }
                None => {
                    label_types.insert(*tweet_id, SafetyLabelMap::default());
                }
            }
        }
        Self {
            label_types,
            label_response,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SafetyLabelType, TweetId};
    use crate::safety_label_source::lookup::RemoteSource;
    use crate::safety_label_source::manhattan::ManhattanSource;
    use crate::safety_label_source::mh_client::{FetchResult, ManhattanLabelFetcher};
    use crate::safety_label_source::twemcache::{CacheRead, TwemcacheSource};
    use crate::safety_label_source::SafetyLabelSource;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tonic::async_trait;
    use xai_cache::{KVCacheError, Key, Value};
    use xai_manhattan::ManhattanError;
    use xai_safety_label_store::types::encode_lkey;

    struct FakeTwemcache {
        results: HashMap<Key, std::result::Result<Option<Value>, KVCacheError>>,
    }

    #[async_trait]
    impl CacheRead for FakeTwemcache {
        async fn multi_get(
            &self,
            _keys: &[Key],
        ) -> HashMap<Key, std::result::Result<Option<Value>, KVCacheError>> {
            self.results.clone()
        }
    }

    struct FakeLabelFetcher {
        items: HashMap<i64, Vec<crate::safety_label_source::codec::RawSafetyLabel>>,
        batch_error: Mutex<Option<ManhattanError>>,
    }

    #[async_trait]
    impl ManhattanLabelFetcher for FakeLabelFetcher {
        async fn fetch_labels(
            &self,
            tweet_ids: &[i64],
        ) -> Result<Vec<FetchResult>, ManhattanError> {
            if let Some(error) = self.batch_error.lock().unwrap().take() {
                return Err(error);
            }
            Ok(tweet_ids
                .iter()
                .map(|id| Ok(self.items.get(id).cloned().unwrap_or_default()))
                .collect())
        }
    }

    fn cache_key(tweet_id: u64) -> Key {
        Key::new(format!("slm_{tweet_id}").into_bytes()).unwrap()
    }

    fn cached_label() -> Value {
        vec![
            0x0b, 0x00, 0x01, 0x00, 0x00, 0x00, 0x14, 0x0c, 0x00, 0x03, 0x0c, 0x5f, 0xff, 0x0d,
            0x69, 0x14, 0x0c, 0x0c, 0x00, 0x00, 0x00, 0x01, 0x01, 0xe7, 0x7c, 0x00, 0x00,
        ]
    }

    fn raw_label(label_type: SafetyLabelType) -> crate::safety_label_source::codec::RawSafetyLabel {
        crate::safety_label_source::codec::RawSafetyLabel {
            lkey: crate::safety_label_source::codec::LkeyBytes(encode_lkey(label_type)),
            mval: crate::safety_label_source::codec::MvalBytes(vec![0x0C, 0x00, 0x04, 0x00, 0x00]),
        }
    }

    async fn hydrate(
        tweet_ids: &[TweetId],
        cache_results: HashMap<Key, std::result::Result<Option<Value>, KVCacheError>>,
        mh_items: HashMap<i64, Vec<crate::safety_label_source::codec::RawSafetyLabel>>,
        batch_error: Option<ManhattanError>,
    ) -> SafetyLabelHydration {
        let twemcache = Arc::new(TwemcacheSource::with_cache(Arc::new(FakeTwemcache {
            results: cache_results,
        })));
        let manhattan = Arc::new(ManhattanSource::new(Arc::new(FakeLabelFetcher {
            items: mh_items,
            batch_error: Mutex::new(batch_error),
        })));
        let source = SafetyLabelSource::new(Arc::new(RemoteSource::new(twemcache, manhattan)));
        let raw: Vec<u64> = tweet_ids.iter().map(|id| id.0).collect();
        SafetyLabelHydration::new(tweet_ids, &source.get(&raw).await)
    }

    #[tokio::test]
    async fn hydrate_keys_results_by_tweet_id() {
        let tweet_ids = vec![TweetId(1), TweetId(2)];
        let result = hydrate(
            &tweet_ids,
            HashMap::from([(cache_key(2), Ok(Some(cached_label())))]),
            HashMap::from([(1, vec![raw_label(SafetyLabelType::NSFW_HIGH_PRECISION)])]),
            None,
        )
        .await;

        assert!(result.label_types[&TweetId(1)].has_label(SafetyLabelType::NSFW_HIGH_PRECISION));
        assert!(result.label_response.contains_key(&TweetId(1)));
        assert!(result.label_response.contains_key(&TweetId(2)));
    }

    #[tokio::test]
    async fn hydrate_fails_open_on_lookup_errors() {
        let tweet_ids = vec![TweetId(1)];
        let result = hydrate(
            &tweet_ids,
            HashMap::new(),
            HashMap::new(),
            Some(ManhattanError::NativeProtocol("decode".into())),
        )
        .await;

        assert!(!result.label_types[&TweetId(1)].has_label(SafetyLabelType::SPAM));
        assert!(!result.label_response.contains_key(&TweetId(1)));
    }

    #[tokio::test]
    async fn hydrate_treats_not_found_as_an_empty_label_map() {
        let tweet_ids = vec![TweetId(1)];
        let result = hydrate(&tweet_ids, HashMap::new(), HashMap::new(), None).await;

        assert!(!result.label_types[&TweetId(1)].has_label(SafetyLabelType::SPAM));
        assert!(result.label_response[&TweetId(1)].labels.is_empty());
    }
}
