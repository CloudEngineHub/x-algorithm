use std::sync::Arc;

use tracing::{error, info, warn};
use xai_manhattan::{NativeManhattanClient, Tenant};

use crate::facts::EntityType;

pub(crate) const MH_LKEY: &str = "0";


pub(crate) const CLASS_NONE: u8 = 0;
pub(crate) const CLASS_LABEL: u8 = 1;
pub(crate) const CLASS_CHALLENGE: u8 = 2;
pub(crate) const CLASS_SUSPEND: u8 = 3;

pub(crate) fn parse_action_class(name: &str) -> Option<u8> {
    match name {
        "label" => Some(CLASS_LABEL),
        "challenge" => Some(CLASS_CHALLENGE),
        "suspend" => Some(CLASS_SUSPEND),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeldKind {
    Placeholder,
    Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldMeta {
    pub class: u8,
    pub kind: HeldKind,
        pub nonce: Option<u64>,
                    pub displaced: Option<(Vec<u8>, u64)>,
}

pub(crate) fn held_entry_meta(raw: &[u8]) -> HeldMeta {
    let json_bytes = zstd::decode_all(raw).unwrap_or_else(|_| raw.to_vec());
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&json_bytes) else {
        return HeldMeta {
            class: CLASS_SUSPEND,
            kind: HeldKind::Outcome,
            nonce: None,
            displaced: None,
        };
    };
    let kind = if v.get("status").is_some() {
        HeldKind::Outcome
    } else {
        HeldKind::Placeholder
    };
    let nonce = v.get("nonce").and_then(serde_json::Value::as_u64);
    let displaced = match (
        v.get("displaced").and_then(serde_json::Value::as_str),
        v.get("displaced_exp").and_then(serde_json::Value::as_u64),
    ) {
        (Some(hex), Some(exp)) => hex_decode(hex).map(|bytes| (bytes, exp)),
        _ => None,
    };
    let class = if let Some(c) = v.get("acted_class").and_then(serde_json::Value::as_u64) {
        c.min(CLASS_SUSPEND as u64) as u8
    } else {
        match v.get("status").and_then(serde_json::Value::as_str) {
            Some("success") => CLASS_SUSPEND,
            Some(_) => CLASS_NONE,
            None => CLASS_SUSPEND,
        }
    };
    HeldMeta {
        class,
        kind,
        nonce,
        displaced,
    }
}

fn carried_class(meta: &HeldMeta, now_secs: u64) -> u8 {
    if meta.kind != HeldKind::Placeholder {
        return CLASS_NONE;
    }
    match (
        displaced_remaining_secs(&meta.displaced, now_secs),
        meta.displaced.as_ref(),
    ) {
        (Some(_), Some((bytes, _))) => held_entry_meta(bytes).class,
        _ => CLASS_NONE,
    }
}

fn fencing_class(meta: &HeldMeta, now_secs: u64) -> u8 {
    match meta.kind {
        HeldKind::Outcome => meta.class,
        HeldKind::Placeholder => carried_class(meta, now_secs),
    }
}

fn fences_write(
    meta: &HeldMeta,
    writer_class: u8,
    writer_nonce: Option<u64>,
    now_secs: u64,
) -> bool {
    match meta.kind {
        HeldKind::Outcome => meta.class > writer_class,
        HeldKind::Placeholder => {
            (writer_nonce.is_none() || meta.nonce != writer_nonce)
                && carried_class(meta, now_secs) > writer_class
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn displaced_remaining_secs(displaced: &Option<(Vec<u8>, u64)>, now_secs: u64) -> Option<u64> {
    match displaced {
        Some((_, exp)) if *exp > now_secs => Some(exp - now_secs),
        _ => None,
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn claim_nonce() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    nanos
        ^ ((std::process::id() as u64) << 32)
        ^ SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

enum SlotView {
    Held(HeldMeta),
    Empty,
        Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupCheck {
        New { claim_nonce: Option<u64> },
                ClassBypass {
        held_class: u8,
        claim_nonce: Option<u64>,
    },
        Duplicate,
}

#[derive(Clone)]
pub struct ManhattanDedupCache {
    client: Arc<NativeManhattanClient>,
    tenant: Tenant,
    ttl_secs: u64,
    skip_ttl_secs: u64,
}

impl ManhattanDedupCache {
    pub fn new(
        client: Arc<NativeManhattanClient>,
        tenant: Tenant,
        ttl_secs: u64,
        skip_ttl_secs: u64,
    ) -> Self {
        Self {
            client,
            tenant,
            ttl_secs,
            skip_ttl_secs,
        }
    }

                                fn pkey(entity_type: EntityType, entity_id: i64) -> String {
        match entity_type {
            EntityType::User => entity_id.to_string(),
            other => format!("{}:{}", other.as_str(), entity_id),
        }
    }

                                                                                pub async fn try_insert(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        incoming_class: u8,
    ) -> DedupCheck {
        let pkey = Self::pkey(entity_type, entity_id);
        match self
            .client
            .get(self.tenant.clone(), [pkey.as_str()], [MH_LKEY])
            .await
        {
            Ok(Some(item)) => {
                let held_exp_secs = item
                    .expires_at()
                    .and_then(|d| u64::try_from(d.timestamp()).ok());
                let raw = item.value().into_bytes();
                let meta = held_entry_meta(&raw);
                if incoming_class <= meta.class {
                    return DedupCheck::Duplicate;
                }
                let displaced = match meta.kind {
                    HeldKind::Outcome if meta.class > CLASS_NONE => {
                        let exp = held_exp_secs.unwrap_or_else(|| now_secs() + self.ttl_secs);
                        Some((raw.as_slice(), exp))
                    }
                    HeldKind::Placeholder => {
                        meta.displaced.as_ref().map(|(b, e)| (b.as_slice(), *e))
                    }
                    _ => None,
                };
                match self.claim(&pkey, incoming_class, displaced).await {
                    None => DedupCheck::ClassBypass {
                        held_class: meta.class,
                        claim_nonce: None,
                    },
                    Some(nonce) => {
                        if self.verify_claim(&pkey, nonce).await {
                            DedupCheck::ClassBypass {
                                held_class: meta.class,
                                claim_nonce: Some(nonce),
                            }
                        } else {
                            info!(
                                pkey,
                                incoming_class,
                                "dedup: bypass claim lost to a concurrent claimant; deduping"
                            );
                            crate::metrics::DEDUP_BYPASS_CLAIM_LOST_TOTAL.inc();
                            DedupCheck::Duplicate
                        }
                    }
                }
            }
            Ok(None) => DedupCheck::New {
                claim_nonce: self.claim(&pkey, incoming_class, None).await,
            },
            Err(e) => {
                error!("Manhattan dedup get failed: {e}; allowing enforcement");
                crate::metrics::MANHATTAN_ERRORS_TOTAL.inc();
                DedupCheck::New { claim_nonce: None }
            }
        }
    }

                                                async fn claim(
        &self,
        pkey: &str,
        acted_class: u8,
        displaced: Option<(&[u8], u64)>,
    ) -> Option<u64> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let nonce = claim_nonce();
        let mut initial =
            serde_json::json!({ "ts": ts, "acted_class": acted_class, "nonce": nonce });
        if let Some((bytes, exp)) = displaced {
            initial["displaced"] = serde_json::Value::String(hex_encode(bytes));
            initial["displaced_exp"] = serde_json::Value::from(exp);
        }
        let deadline = crate::manhattan::ttl_nanos_from_secs(self.ttl_secs);
        match self
            .client
            .put_with_ttl(
                self.tenant.clone(),
                [pkey],
                [MH_LKEY],
                initial.to_string().into_bytes(),
                deadline,
            )
            .await
        {
            Ok(()) => Some(nonce),
            Err(e) => {
                error!("Manhattan dedup put failed: {e}; allowing enforcement");
                crate::metrics::MANHATTAN_ERRORS_TOTAL.inc();
                None
            }
        }
    }

                                async fn verify_claim(&self, pkey: &str, nonce: u64) -> bool {
        match self
            .client
            .get(self.tenant.clone(), [pkey], [MH_LKEY])
            .await
        {
            Ok(Some(item)) => held_entry_meta(&item.value().into_bytes()).nonce == Some(nonce),
            Ok(None) => false,
            Err(e) => {
                warn!("Manhattan dedup verify get failed: {e}; allowing enforcement");
                crate::metrics::MANHATTAN_ERRORS_TOTAL.inc();
                true
            }
        }
    }

                                    async fn slot_view(&self, pkey: &str) -> SlotView {
        match self
            .client
            .get(self.tenant.clone(), [pkey], [MH_LKEY])
            .await
        {
            Ok(Some(item)) => SlotView::Held(held_entry_meta(&item.value().into_bytes())),
            Ok(None) => SlotView::Empty,
            Err(e) => {
                warn!("Manhattan dedup fence get failed: {e}; dropping mutation (fail-safe)");
                crate::metrics::MANHATTAN_ERRORS_TOTAL.inc();
                SlotView::Unknown
            }
        }
    }

        fn count_fenced(pkey: &str, op: &str, held_class: u8, writer_class: u8) {
        info!(
            pkey,
            op, held_class, writer_class, "dedup: write fenced (stronger terminal outcome held)"
        );
        crate::metrics::DEDUP_WRITE_FENCED_TOTAL
            .with_label_values(&[op, &held_class.to_string(), &writer_class.to_string()])
            .inc();
    }

                                                                                                                        async fn restore_displaced(
        &self,
        pkey: &str,
        meta: &HeldMeta,
        writer_nonce: Option<u64>,
        op: &str,
    ) -> bool {
        if meta.kind != HeldKind::Placeholder
            || writer_nonce.is_none()
            || meta.nonce != writer_nonce
        {
            return false;
        }
        let Some(remaining) = displaced_remaining_secs(&meta.displaced, now_secs()) else {
            return false;
        };
        let (bytes, _) = meta
            .displaced
            .as_ref()
            .expect("displaced_remaining_secs implies displaced");
        if self.update_with_ttl(pkey, bytes, remaining).await {
            info!(
                pkey,
                op, remaining, "dedup: restored displaced hold (bypassing decision did not act)"
            );
            crate::metrics::DEDUP_DISPLACED_RESTORED_TOTAL
                .with_label_values(&[op])
                .inc();
        } else {
            warn!(
                pkey,
                op, "dedup: displaced-hold restore PUT failed; claim left in place (over-hold)"
            );
        }
        true
    }

                                                                        pub async fn update(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        value: &[u8],
        acted_class: u8,
        claim_nonce: Option<u64>,
    ) {
        let pkey = Self::pkey(entity_type, entity_id);
        if acted_class < CLASS_SUSPEND {
            match self.slot_view(&pkey).await {
                SlotView::Held(meta) => {
                    let now = now_secs();
                    if fences_write(&meta, acted_class, claim_nonce, now) {
                        Self::count_fenced(&pkey, "acted", fencing_class(&meta, now), acted_class);
                        return;
                    }
                }
                SlotView::Empty => {}
                SlotView::Unknown => return,
            }
        }
        self.update_with_ttl(&pkey, value, self.ttl_secs).await;
    }

                                                                                                        pub async fn update_skip(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        value: &[u8],
        acted_class: u8,
        claim_nonce: Option<u64>,
    ) {
        let pkey = Self::pkey(entity_type, entity_id);
        match self.slot_view(&pkey).await {
            SlotView::Held(meta) => {
                let now = now_secs();
                if fences_write(&meta, acted_class, claim_nonce, now) {
                    Self::count_fenced(&pkey, "skip", fencing_class(&meta, now), acted_class);
                    return;
                }
                if self
                    .restore_displaced(&pkey, &meta, claim_nonce, "skip")
                    .await
                {
                    return;
                }
            }
            SlotView::Empty => {}
            SlotView::Unknown => return,
        }
        self.update_with_ttl(&pkey, value, self.skip_ttl_secs).await;
    }

                async fn update_with_ttl(&self, pkey: &str, value: &[u8], ttl_secs: u64) -> bool {
        let deadline = crate::manhattan::ttl_nanos_from_secs(ttl_secs);
        match self
            .client
            .put_with_ttl(
                self.tenant.clone(),
                [pkey],
                [MH_LKEY],
                value.to_vec(),
                deadline,
            )
            .await
        {
            Ok(()) => true,
            Err(e) => {
                warn!("Manhattan dedup update failed: {e}");
                crate::metrics::MANHATTAN_ERRORS_TOTAL.inc();
                false
            }
        }
    }

                                                                        pub async fn invalidate(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        claim_nonce: Option<u64>,
    ) {
        let pkey = Self::pkey(entity_type, entity_id);
        match self.slot_view(&pkey).await {
            SlotView::Held(meta) => {
                if meta.kind == HeldKind::Outcome {
                    info!(
                        pkey,
                        held_class = meta.class,
                        "dedup: invalidate left terminal outcome in place"
                    );
                    return;
                }
                if claim_nonce.is_none() || meta.nonce != claim_nonce {
                    info!(pkey, "dedup: invalidate left foreign claim in place");
                    return;
                }
                if self
                    .restore_displaced(&pkey, &meta, claim_nonce, "invalidate")
                    .await
                {
                    return;
                }
            }
            SlotView::Empty => {}
            SlotView::Unknown => return,
        }
        if let Err(e) = self
            .client
            .delete(self.tenant.clone(), [pkey.as_str()], [MH_LKEY])
            .await
        {
            warn!("Manhattan dedup delete failed: {e}");
            crate::metrics::MANHATTAN_ERRORS_TOTAL.inc();
        }
    }

                pub async fn get_with_ttl(
        &self,
        entity_type: EntityType,
        entity_id: i64,
    ) -> Option<(Vec<u8>, i64)> {
        let pkey = Self::pkey(entity_type, entity_id);
        match self
            .client
            .get(self.tenant.clone(), [pkey.as_str()], [MH_LKEY])
            .await
        {
            Ok(Some(item)) => {
                let ttl_secs = crate::manhattan::remaining_ttl_secs(
                    item.expires_at()
                        .map(|d| d.timestamp_nanos_opt().unwrap_or(0) as u64),
                );
                Some((item.value().into_bytes(), ttl_secs))
            }
            Ok(None) => None,
            Err(e) => {
                warn!("Manhattan dedup get failed: {e}");
                crate::metrics::MANHATTAN_ERRORS_TOTAL.inc();
                None
            }
        }
    }

                    pub async fn peek(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        incoming_class: u8,
    ) -> Option<u8> {
        let pkey = Self::pkey(entity_type, entity_id);
        match self
            .client
            .get(self.tenant.clone(), [pkey.as_str()], [MH_LKEY])
            .await
        {
            Ok(Some(item)) => {
                let meta = held_entry_meta(&item.value().into_bytes());
                (incoming_class <= meta.class).then_some(meta.class)
            }
            Ok(None) => None,
            Err(e) => {
                warn!("Manhattan dedup peek failed: {e}");
                crate::metrics::MANHATTAN_ERRORS_TOTAL.inc();
                None
            }
        }
    }
}

#[async_trait::async_trait]
pub trait DedupSlot: Send + Sync {
    async fn try_insert(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        incoming_class: u8,
    ) -> DedupCheck;
    async fn peek(&self, entity_type: EntityType, entity_id: i64, incoming_class: u8)
    -> Option<u8>;
    async fn update(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        value: &[u8],
        acted_class: u8,
        claim_nonce: Option<u64>,
    );
    async fn update_skip(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        value: &[u8],
        acted_class: u8,
        claim_nonce: Option<u64>,
    );
    async fn invalidate(&self, entity_type: EntityType, entity_id: i64, claim_nonce: Option<u64>);
}

#[async_trait::async_trait]
impl DedupSlot for ManhattanDedupCache {
    async fn try_insert(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        incoming_class: u8,
    ) -> DedupCheck {
        ManhattanDedupCache::try_insert(self, entity_type, entity_id, incoming_class).await
    }
    async fn peek(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        incoming_class: u8,
    ) -> Option<u8> {
        ManhattanDedupCache::peek(self, entity_type, entity_id, incoming_class).await
    }
    async fn update(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        value: &[u8],
        acted_class: u8,
        claim_nonce: Option<u64>,
    ) {
        ManhattanDedupCache::update(
            self,
            entity_type,
            entity_id,
            value,
            acted_class,
            claim_nonce,
        )
        .await
    }
    async fn update_skip(
        &self,
        entity_type: EntityType,
        entity_id: i64,
        value: &[u8],
        acted_class: u8,
        claim_nonce: Option<u64>,
    ) {
        ManhattanDedupCache::update_skip(
            self,
            entity_type,
            entity_id,
            value,
            acted_class,
            claim_nonce,
        )
        .await
    }
    async fn invalidate(&self, entity_type: EntityType, entity_id: i64, claim_nonce: Option<u64>) {
        ManhattanDedupCache::invalidate(self, entity_type, entity_id, claim_nonce).await
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

                    type Slot = (u8, bool, Option<u64>);

    #[derive(Default)]
    pub(crate) struct RecordingDedup {
        pub calls: Mutex<Vec<String>>,
                pub slots: Mutex<HashMap<(String, i64), Slot>>,
        next_nonce: std::sync::atomic::AtomicU64,
    }

    impl RecordingDedup {
        pub(crate) fn hold(&self, entity_type: EntityType, id: i64, class: u8) {
            self.slots
                .lock()
                .unwrap()
                .insert((entity_type.as_str().into(), id), (class, false, None));
        }

        pub(crate) fn take_calls(&self) -> Vec<String> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }

        fn log(&self, s: String) {
            self.calls.lock().unwrap().push(s);
        }
    }

    #[async_trait::async_trait]
    impl DedupSlot for RecordingDedup {
        async fn try_insert(&self, et: EntityType, id: i64, incoming: u8) -> DedupCheck {
            let key = (et.as_str().to_owned(), id);
            let mut slots = self.slots.lock().unwrap();
            let nonce = self
                .next_nonce
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            let out = match slots.get(&key) {
                Some((held, _, _)) if incoming <= *held => DedupCheck::Duplicate,
                Some((held, _, _)) => {
                    let held = *held;
                    slots.insert(key, (incoming, true, Some(nonce)));
                    DedupCheck::ClassBypass {
                        held_class: held,
                        claim_nonce: Some(nonce),
                    }
                }
                None => {
                    slots.insert(key, (incoming, true, Some(nonce)));
                    DedupCheck::New {
                        claim_nonce: Some(nonce),
                    }
                }
            };
            drop(slots);
            let tag = match out {
                DedupCheck::New { .. } => "new",
                DedupCheck::ClassBypass { .. } => "bypass",
                DedupCheck::Duplicate => "duplicate",
            };
            self.log(format!("try_insert(class={incoming})={tag}"));
            out
        }

        async fn peek(&self, et: EntityType, id: i64, incoming: u8) -> Option<u8> {
            let held = self
                .slots
                .lock()
                .unwrap()
                .get(&(et.as_str().to_owned(), id))
                .map(|(c, _, _)| *c);
            let out = held.filter(|h| incoming <= *h);
            self.log(format!("peek(class={incoming})={out:?}"));
            out
        }

        async fn update(&self, et: EntityType, id: i64, _v: &[u8], class: u8, n: Option<u64>) {
            self.slots
                .lock()
                .unwrap()
                .insert((et.as_str().into(), id), (class, false, None));
            self.log(format!("update(class={class},own_claim={})", n.is_some()));
        }

        async fn update_skip(&self, et: EntityType, id: i64, _v: &[u8], class: u8, n: Option<u64>) {
            self.slots
                .lock()
                .unwrap()
                .insert((et.as_str().into(), id), (CLASS_NONE, false, None));
            self.log(format!(
                "update_skip(class={class},own_claim={})",
                n.is_some()
            ));
        }

        async fn invalidate(&self, et: EntityType, id: i64, n: Option<u64>) {
            let key = (et.as_str().to_owned(), id);
            let mut slots = self.slots.lock().unwrap();
            if let Some((_, true, held)) = slots.get(&key)
                && n.is_some()
                && *held == n
            {
                slots.remove(&key);
            }
            drop(slots);
            self.log(format!("invalidate(own_claim={})", n.is_some()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zstd_json(v: serde_json::Value) -> Vec<u8> {
        zstd::encode_all(v.to_string().as_bytes(), 3).unwrap()
    }

            fn held_action_class(raw: &[u8]) -> u8 {
        held_entry_meta(raw).class
    }

    #[test]
    fn held_class_reads_acted_class_field() {
        for class in [CLASS_NONE, CLASS_LABEL, CLASS_CHALLENGE, CLASS_SUSPEND] {
            let entry = serde_json::json!({ "status": "success", "acted_class": class });
            assert_eq!(held_action_class(&zstd_json(entry.clone())), class);
            assert_eq!(held_action_class(entry.to_string().as_bytes()), class);
        }
        let entry = serde_json::json!({ "acted_class": 200 });
        assert_eq!(
            held_action_class(entry.to_string().as_bytes()),
            CLASS_SUSPEND
        );
        let claim = serde_json::json!({ "ts": 1, "acted_class": CLASS_LABEL });
        assert_eq!(held_action_class(claim.to_string().as_bytes()), CLASS_LABEL);
    }

    #[test]
    fn held_class_legacy_entries() {
        let legacy_success = serde_json::json!({ "status": "success", "topic": "t" });
        assert_eq!(held_action_class(&zstd_json(legacy_success)), CLASS_SUSPEND);
        let legacy_skip = serde_json::json!({ "status": "dedup_skipped" });
        assert_eq!(held_action_class(&zstd_json(legacy_skip)), CLASS_NONE);
        let legacy_dry = serde_json::json!({ "status": "dry_run" });
        assert_eq!(held_action_class(&zstd_json(legacy_dry)), CLASS_NONE);
        let legacy_claim = serde_json::json!({ "ts": 12345 });
        assert_eq!(
            held_action_class(legacy_claim.to_string().as_bytes()),
            CLASS_SUSPEND
        );
        assert_eq!(held_action_class(b"\x02not json"), CLASS_SUSPEND);
    }

    #[test]
    fn held_meta_distinguishes_placeholders_from_outcomes() {
        let claim = serde_json::json!({ "ts": 1, "acted_class": CLASS_SUSPEND });
        let meta = held_entry_meta(claim.to_string().as_bytes());
        assert_eq!(
            (meta.class, meta.kind),
            (CLASS_SUSPEND, HeldKind::Placeholder)
        );
        assert_eq!(meta.displaced, None);
        let legacy_claim = serde_json::json!({ "ts": 12345 });
        let meta = held_entry_meta(legacy_claim.to_string().as_bytes());
        assert_eq!(
            (meta.class, meta.kind),
            (CLASS_SUSPEND, HeldKind::Placeholder)
        );
        assert_eq!(meta.nonce, None);
        let acted = serde_json::json!({ "status": "success", "acted_class": CLASS_LABEL });
        let meta = held_entry_meta(&zstd_json(acted));
        assert_eq!((meta.class, meta.kind), (CLASS_LABEL, HeldKind::Outcome));
        let legacy_success = serde_json::json!({ "status": "success" });
        let meta = held_entry_meta(&zstd_json(legacy_success));
        assert_eq!((meta.class, meta.kind), (CLASS_SUSPEND, HeldKind::Outcome));
        let legacy_skip = serde_json::json!({ "status": "high_follower_count" });
        let meta = held_entry_meta(&zstd_json(legacy_skip));
        assert_eq!((meta.class, meta.kind), (CLASS_NONE, HeldKind::Outcome));
        let meta = held_entry_meta(b"\x02not json");
        assert_eq!((meta.class, meta.kind), (CLASS_SUSPEND, HeldKind::Outcome));
    }

    #[test]
    fn bypass_claim_roundtrips_nonce_and_displaced_hold() {
        let displaced_entry = zstd_json(serde_json::json!({
            "status": "success", "acted_class": CLASS_LABEL, "topic": "t"
        }));
        let claim = serde_json::json!({
            "ts": 1,
            "acted_class": CLASS_SUSPEND,
            "nonce": 42u64,
            "displaced": hex_encode(&displaced_entry),
            "displaced_exp": 2_000_000_000u64,
        });
        let meta = held_entry_meta(claim.to_string().as_bytes());
        assert_eq!(
            (meta.class, meta.kind),
            (CLASS_SUSPEND, HeldKind::Placeholder)
        );
        assert_eq!(meta.nonce, Some(42));
        let (bytes, exp) = meta.displaced.expect("displaced hold must roundtrip");
        assert_eq!(exp, 2_000_000_000);
        let restored = held_entry_meta(&bytes);
        assert_eq!(
            (restored.class, restored.kind),
            (CLASS_LABEL, HeldKind::Outcome)
        );
        let bad = serde_json::json!({ "ts": 1, "displaced": "zz", "displaced_exp": 5u64 });
        assert_eq!(held_entry_meta(bad.to_string().as_bytes()).displaced, None);
    }

    #[test]
    fn hex_roundtrip() {
        let bytes: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        assert_eq!(hex_decode(&hex_encode(&bytes)), Some(bytes));
        assert_eq!(hex_decode(""), Some(vec![]));
        assert_eq!(hex_decode("abc"), None, "odd length");
        assert_eq!(hex_decode("zz"), None, "non-hex");
    }

    #[test]
    fn displaced_remaining_only_for_unexpired_holds() {
        let displaced = Some((vec![1u8], 1_000u64));
        assert_eq!(displaced_remaining_secs(&displaced, 400), Some(600));
        assert_eq!(displaced_remaining_secs(&displaced, 1_000), None, "expired");
        assert_eq!(displaced_remaining_secs(&displaced, 2_000), None, "expired");
        assert_eq!(displaced_remaining_secs(&None, 0), None, "absent");
    }

    #[test]
    fn claim_nonces_are_distinct() {
        assert_ne!(claim_nonce(), claim_nonce());
    }

        fn outcome_meta(class: u8) -> HeldMeta {
        HeldMeta {
            class,
            kind: HeldKind::Outcome,
            nonce: None,
            displaced: None,
        }
    }

                fn claim_meta(class: u8, nonce: u64, carry: Option<(u8, u64)>) -> HeldMeta {
        HeldMeta {
            class,
            kind: HeldKind::Placeholder,
            nonce: Some(nonce),
            displaced: carry.map(|(carry_class, carry_exp)| {
                let entry = serde_json::json!({ "status": "success", "acted_class": carry_class });
                (zstd_json(entry), carry_exp)
            }),
        }
    }

    #[test]
    fn write_fence_blocks_strictly_stronger_outcomes() {
        let now = 1_000u64;
        assert!(fences_write(
            &outcome_meta(CLASS_SUSPEND),
            CLASS_LABEL,
            None,
            now
        ));
        assert!(fences_write(
            &outcome_meta(CLASS_SUSPEND),
            CLASS_NONE,
            None,
            now
        ));
        assert!(fences_write(
            &outcome_meta(CLASS_LABEL),
            CLASS_NONE,
            None,
            now
        ));
        assert!(!fences_write(
            &outcome_meta(CLASS_SUSPEND),
            CLASS_SUSPEND,
            None,
            now
        ));
        assert!(!fences_write(
            &outcome_meta(CLASS_LABEL),
            CLASS_SUSPEND,
            None,
            now
        ));
        assert!(!fences_write(
            &outcome_meta(CLASS_NONE),
            CLASS_NONE,
            None,
            now
        ));
    }

    #[test]
    fn write_fence_placeholders_own_never_foreign_by_carry() {
        let now = 1_000u64;
        let exp = 2_000u64;
        let own = claim_meta(CLASS_SUSPEND, 42, Some((CLASS_CHALLENGE, exp)));
        assert!(!fences_write(&own, CLASS_NONE, Some(42), now));
        assert!(!fences_write(&own, CLASS_LABEL, Some(42), now));
        assert!(
            fences_write(&own, CLASS_NONE, Some(7), now),
            "foreign nonce"
        );
        assert!(fences_write(&own, CLASS_LABEL, Some(7), now));
        assert!(
            fences_write(&own, CLASS_NONE, None, now),
            "nonce-less writer"
        );
        assert!(
            !fences_write(&own, CLASS_CHALLENGE, Some(7), now),
            "equal-class write supersedes the carry"
        );
        assert!(!fences_write(&own, CLASS_SUSPEND, Some(7), now));
        let bare = claim_meta(CLASS_SUSPEND, 42, None);
        assert!(!fences_write(&bare, CLASS_NONE, Some(7), now));
        assert!(!fences_write(&bare, CLASS_NONE, None, now));
        let expired = claim_meta(CLASS_SUSPEND, 42, Some((CLASS_CHALLENGE, 500)));
        assert!(!fences_write(&expired, CLASS_NONE, Some(7), now));
    }

    #[test]
    fn carried_class_ranks_unexpired_carries_only() {
        let now = 1_000u64;
        let meta = claim_meta(CLASS_SUSPEND, 1, Some((CLASS_LABEL, 2_000)));
        assert_eq!(carried_class(&meta, now), CLASS_LABEL);
        let expired = claim_meta(CLASS_SUSPEND, 1, Some((CLASS_LABEL, 999)));
        assert_eq!(carried_class(&expired, now), CLASS_NONE);
        assert_eq!(
            carried_class(&claim_meta(CLASS_SUSPEND, 1, None), now),
            CLASS_NONE
        );
        assert_eq!(carried_class(&outcome_meta(CLASS_SUSPEND), now), CLASS_NONE);
        assert_eq!(
            fencing_class(&outcome_meta(CLASS_CHALLENGE), now),
            CLASS_CHALLENGE
        );
        assert_eq!(
            fencing_class(
                &claim_meta(CLASS_SUSPEND, 1, Some((CLASS_LABEL, 2_000))),
                now
            ),
            CLASS_LABEL
        );
    }

    #[test]
    fn parse_action_class_names() {
        assert_eq!(parse_action_class("label"), Some(CLASS_LABEL));
        assert_eq!(parse_action_class("challenge"), Some(CLASS_CHALLENGE));
        assert_eq!(parse_action_class("suspend"), Some(CLASS_SUSPEND));
        assert_eq!(parse_action_class("Suspend"), None);
        assert_eq!(parse_action_class(""), None);
    }

    #[test]
    fn pkey_keeps_user_bare_and_namespaces_others() {
        assert_eq!(ManhattanDedupCache::pkey(EntityType::User, 123), "123");
        assert_eq!(ManhattanDedupCache::pkey(EntityType::Post, 123), "post:123");
        assert_ne!(
            ManhattanDedupCache::pkey(EntityType::User, 123),
            ManhattanDedupCache::pkey(EntityType::Post, 123),
        );
    }
}
