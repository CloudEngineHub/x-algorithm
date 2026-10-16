use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct RecordingReceiver {
    counters: Mutex<HashMap<String, u64>>,
    vm_observations: Mutex<HashMap<String, u64>>,
}

impl RecordingReceiver {
    pub(crate) fn counter(&self, key: &str) -> u64 {
        *self.counters.lock().unwrap().get(key).unwrap_or(&0)
    }

    pub(crate) fn vm_observations(&self, key: &str) -> u64 {
        *self.vm_observations.lock().unwrap().get(key).unwrap_or(&0)
    }
}

fn key(name: &str, scopes: &[(&str, &str)]) -> String {
    let mut key = name.to_string();
    for (k, v) in scopes {
        key.push('|');
        key.push_str(k);
        key.push('=');
        key.push_str(v);
    }
    key
}

impl xai_stats_receiver::StatsReceiverExt for RecordingReceiver {
    fn incr(&self, name: &str, scopes: &[(&str, &str)], value: u64) {
        *self
            .counters
            .lock()
            .unwrap()
            .entry(key(name, scopes))
            .or_default() += value;
    }
    fn observe(
        &self,
        _: &str,
        _: &[(&str, &str)],
        _: f64,
        _: xai_stats_receiver::HistogramBuckets,
    ) {
    }
    fn observe_expo(&self, _: &str, _: &[(&str, &str)], _: f64) {}
    fn observe_vm(&self, name: &str, scopes: &[(&str, &str)], _: f64) {
        *self
            .vm_observations
            .lock()
            .unwrap()
            .entry(key(name, scopes))
            .or_default() += 1;
    }
    fn gauge(&self, _: &str, _: &[(&str, &str)], _: f64) {}
}
