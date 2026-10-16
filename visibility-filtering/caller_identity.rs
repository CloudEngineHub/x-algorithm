use std::cell::Cell;
use std::time::Instant;
use tonic::Request;
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;
use xai_stats_receiver::{global_stats_receiver, StatsReceiverExt};

const REQUESTS_BY_CALLER: &str = "vf_requests_by_caller";
const LATENCY_BY_CALLER_MS: &str = "vf_request_latency_by_caller_ms";
const S2S_IDENTITY_PREFIX: &str = "twtr:svc:";
const UNKNOWN_IDENTITY: &str = "unknown";

#[derive(Clone, Copy, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum Endpoint {
    FilterTweets,
    EvaluateTweets,
    GetSafetyLabels,
}

#[derive(Clone, Copy, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum Outcome {
    Success,
    Failure,
    Cancelled,
}

pub(crate) struct CallerRequest {
    identity: Option<String>,
    endpoint: Endpoint,
    outcome: Cell<Outcome>,
    start: Instant,
}

impl CallerRequest {
    pub(crate) fn mark_success(&self) {
        self.outcome.set(Outcome::Success);
    }

    pub(crate) fn mark_failure(&self) {
        self.outcome.set(Outcome::Failure);
    }

    fn emit(&self, sr: &dyn StatsReceiverExt) {
        let caller_identity = self.identity.as_deref().unwrap_or(UNKNOWN_IDENTITY);
        let outcome = self.outcome.get();
        sr.incr(
            REQUESTS_BY_CALLER,
            &[
                ("caller_identity", caller_identity),
                ("rpc", self.endpoint.into()),
                ("outcome", outcome.into()),
            ],
            1,
        );
        if matches!(outcome, Outcome::Success) {
            sr.observe_vm(
                LATENCY_BY_CALLER_MS,
                &[
                    ("caller_identity", caller_identity),
                    ("rpc", self.endpoint.into()),
                ],
                self.start.elapsed().as_secs_f64() * 1000.0,
            );
        }
    }
}

impl Drop for CallerRequest {
    fn drop(&mut self) {
        if let Some(sr) = global_stats_receiver() {
            self.emit(&*sr);
        }
    }
}

#[must_use = "the request is counted when the guard drops"]
pub(crate) fn record<T>(endpoint: Endpoint, request: &Request<T>) -> CallerRequest {
    let identity = request
        .peer_certs()
        .and_then(|certs| identity_from_der(certs.first()?.as_ref()));
    CallerRequest {
        identity,
        endpoint,
        outcome: Cell::new(Outcome::Cancelled),
        start: Instant::now(),
    }
}

fn identity_from_der(der: &[u8]) -> Option<String> {
    let (_, cert) = parse_x509_certificate(der).ok()?;
    let san = cert.subject_alternative_name().ok().flatten();
    san.and_then(|san| san.value.general_names.iter().find_map(s2s_identity_uri))
        .or_else(|| {
            cert.subject()
                .iter_common_name()
                .filter_map(|cn| cn.as_str().ok())
                .find(|cn| cn.starts_with(S2S_IDENTITY_PREFIX))
        })
        .map(str::to_string)
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "GeneralName is x509-parser's SAN enum; only a URI name carries an S2S identity, so every other kind is skipped, including any the crate adds"
)]
fn s2s_identity_uri<'a>(name: &GeneralName<'a>) -> Option<&'a str> {
    match name {
        GeneralName::URI(uri) if uri.starts_with(S2S_IDENTITY_PREFIX) => Some(*uri),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;
    use xai_stats_receiver::HistogramBuckets;

    const S2S_IDENTITY: &str = "twtr:svc:example-caller:example-caller:prod:atla";

    const CERT_URI_SAN: &str = "-----BEGIN CERTIFICATE-----
MIICAzCCAaigAwIBAgIURhTmMlB+9nkf4PAvEXeJ3Kq1K9MwCgYIKoZIzj0EAwIw
NzE1MDMGA1UEAwwsdHd0cjpzdmM6b3RoZXItY2FsbGVyOm90aGVyLWNhbGxlcjpw
cm9kOmF0bGEwHhcNMjYwOTIyMjIxOTU2WhcNMzYwOTE5MjIxOTU2WjA3MTUwMwYD
VQQDDCx0d3RyOnN2YzpvdGhlci1jYWxsZXI6b3RoZXItY2FsbGVyOnByb2Q6YXRs
YTBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABF7KeC/VQJ3Yn3orD0xvIdG6z8Ay
Ukh8pLvT5MuVMdYE6pQ0n4ooiu9huvuQOPdWobJwPuWswnEtXiOks4ctmwajgZEw
gY4wHQYDVR0OBBYEFIndK/WcKSmcOjoFNpiVSmQeq2SKMB8GA1UdIwQYMBaAFInd
K/WcKSmcOjoFNpiVSmQeq2SKMA8GA1UdEwEB/wQFMAMBAf8wOwYDVR0RBDQwMoYw
dHd0cjpzdmM6ZXhhbXBsZS1jYWxsZXI6ZXhhbXBsZS1jYWxsZXI6cHJvZDphdGxh
MAoGCCqGSM49BAMCA0kAMEYCIQCq4/pqYd4TawCBzhTjbMRm4COd09WL6XeSRv07
nq57nAIhAJYaQVHBaSZnA8xxRAUmCw7WxFI3XhTb5rPVQIvX9xBF
-----END CERTIFICATE-----";

    const CERT_CN_ONLY: &str = "-----BEGIN CERTIFICATE-----
MIIByjCCAXGgAwIBAgIUS3R2FofIbToamGbkyqtjD6GjgpkwCgYIKoZIzj0EAwIw
OzE5MDcGA1UEAwwwdHd0cjpzdmM6ZXhhbXBsZS1jYWxsZXI6ZXhhbXBsZS1jYWxs
ZXI6cHJvZDphdGxhMB4XDTI2MDkyMjIyMTk1NloXDTM2MDkxOTIyMTk1NlowOzE5
MDcGA1UEAwwwdHd0cjpzdmM6ZXhhbXBsZS1jYWxsZXI6ZXhhbXBsZS1jYWxsZXI6
cHJvZDphdGxhMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEaN/eKVSe1oNOwMcg
ICsbI7vnZuLZLfaqi5e5bVljMZE6nCZW44beswGHXAdFlqEnmKCWxurkVbo4xuz7
v38WyaNTMFEwHQYDVR0OBBYEFFDk0CzlybHpk3mVikQtrL7IjWhIMB8GA1UdIwQY
MBaAFFDk0CzlybHpk3mVikQtrL7IjWhIMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZI
zj0EAwIDRwAwRAIgWYxRlwLH+EbqUZxbghkCjM88SEsb1tMtq+JEjnBlBKACICtA
08IB38v4cuLkg58SeovYLjgLo16xlnR2bz4T/CYo
-----END CERTIFICATE-----";

    const CERT_NON_S2S: &str = "-----BEGIN CERTIFICATE-----
MIIBozCCAUmgAwIBAgIUCY7TpfIvhXIqCCfjKC0UTaM7aaAwCgYIKoZIzj0EAwIw
FzEVMBMGA1UEAwwMZXhhbXBsZS1ob3N0MB4XDTI2MDkyMjIyMTk1NloXDTM2MDkx
OTIyMTk1NlowFzEVMBMGA1UEAwwMZXhhbXBsZS1ob3N0MFkwEwYHKoZIzj0CAQYI
KoZIzj0DAQcDQgAECS4vwZjw0hyoUYkX43SKxfBt00fsz4zIC02HjLkMiINZND6c
HtKcKKZVvQFJObhp5SdWHxDQ41RybVv4kpRovaNzMHEwHQYDVR0OBBYEFH/8gm+R
TQ+N41YHE19IP4oRJ4RgMB8GA1UdIwQYMBaAFH/8gm+RTQ+N41YHE19IP4oRJ4Rg
MA8GA1UdEwEB/wQFMAMBAf8wHgYDVR0RBBcwFYYTaHR0cHM6Ly9leGFtcGxlLmNv
bTAKBggqhkjOPQQDAgNIADBFAiEA5G5uc6pIQz8w08GlYl7xnShijhCe129+9lbn
tMYHDu0CIG/uxg3z7cGl6/7g6seBJJ7qqxJNLaQBRlMu1Ntki9Q7
-----END CERTIFICATE-----";

    fn identity(pem: &str) -> Option<String> {
        let (_, pem) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap();
        identity_from_der(&pem.contents)
    }

    #[test]
    fn identity_prefers_uri_san_over_common_name() {
        assert_eq!(identity(CERT_URI_SAN).as_deref(), Some(S2S_IDENTITY));
    }

    #[test]
    fn identity_falls_back_to_common_name() {
        assert_eq!(identity(CERT_CN_ONLY).as_deref(), Some(S2S_IDENTITY));
    }

    #[test]
    fn non_s2s_certificate_has_no_identity() {
        assert_eq!(identity(CERT_NON_S2S), None);
    }

    #[test]
    fn dashboard_generator_pins_the_requests_by_caller_metric() {
        let cargo = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/dashboard.py");
        let ws = "crates/x-product/xai-visibility-filtering-service/scripts/dashboard.py";
        let path = if Path::new(cargo).exists() { cargo } else { ws };
        let dashboard = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        assert!(dashboard.contains(&format!(
            "REQUESTS_BY_CALLER_METRIC = \"{REQUESTS_BY_CALLER}\""
        )));
        assert!(dashboard.contains(&format!(
            "VF_LATENCY_BY_CALLER_METRIC = \"{LATENCY_BY_CALLER_MS}\""
        )));
        let [success, failure, cancelled] =
            [Outcome::Success, Outcome::Failure, Outcome::Cancelled].map(<&str>::from);
        assert!(dashboard.contains(&format!(
            "REQUESTS_BY_CALLER_SUCCESS_FILTER = 'outcome=\"{success}\"'"
        )));
        assert!(dashboard.contains(&format!(
            "REQUESTS_BY_CALLER_NON_SUCCESS_FILTER = 'outcome=~\"{failure}|{cancelled}\"'"
        )));
    }

    #[test]
    fn caller_request_defaults_to_cancelled_until_marked() {
        let request = record(Endpoint::GetSafetyLabels, &Request::new(()));
        assert!(matches!(request.outcome.get(), Outcome::Cancelled));
        request.mark_failure();
        assert!(matches!(request.outcome.get(), Outcome::Failure));
        request.mark_success();
        assert!(matches!(request.outcome.get(), Outcome::Success));
    }

    #[derive(Default)]
    struct Latencies(Mutex<Vec<(String, String)>>);

    impl StatsReceiverExt for Latencies {
        fn incr(&self, _: &str, _: &[(&str, &str)], _: u64) {}
        fn observe(&self, _: &str, _: &[(&str, &str)], _: f64, _: HistogramBuckets) {}
        fn observe_expo(&self, _: &str, _: &[(&str, &str)], _: f64) {}
        fn observe_vm(&self, name: &str, scopes: &[(&str, &str)], _: f64) {
            let labels = scopes
                .iter()
                .map(|(label, tag)| format!("{label}={tag}"))
                .collect::<Vec<_>>()
                .join(",");
            self.0.lock().unwrap().push((name.to_string(), labels));
        }
        fn gauge(&self, _: &str, _: &[(&str, &str)], _: f64) {}
    }

    #[test]
    fn only_a_successful_request_observes_its_latency_by_caller() {
        let latencies = Latencies::default();
        let request = record(Endpoint::GetSafetyLabels, &Request::new(()));
        request.emit(&latencies);
        request.mark_failure();
        request.emit(&latencies);
        assert!(latencies.0.lock().unwrap().is_empty());
        request.mark_success();
        request.emit(&latencies);
        assert_eq!(
            *latencies.0.lock().unwrap(),
            [(
                LATENCY_BY_CALLER_MS.to_string(),
                "caller_identity=unknown,rpc=get_safety_labels".to_string()
            )]
        );
    }
}
