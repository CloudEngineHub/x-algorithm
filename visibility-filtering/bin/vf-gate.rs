#![deny(clippy::allow_attributes, clippy::allow_attributes_without_reason)]

use std::io::{Read, Write};
use xai_visibility_filtering::vf_client::XaiVfClient;
use xai_visibility_filtering_service::dark_traffic_setup::staging_tls_domain;
use xai_visibility_filtering_service::verdict_gate::{parse_cases, GRPC_PORT};
use xai_x_rpc::grpc_client::{ChannelBuilder, TlsMode};

const CALL_TIMEOUT_MS: u64 = 5_000;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let cases = parse_cases(&input)?;
    let tls = TlsMode::mtls_from_env()?
        .with_domain_override(staging_tls_domain(&std::env::var("DATACENTER")?));
    let channel = ChannelBuilder::new("vf-gate")
        .destination(format!("https://localhost:{GRPC_PORT}"))
        .tls(tls)
        .build()
        .await?;
    let client = XaiVfClient::from_channel(channel).with_timeout_ms(CALL_TIMEOUT_MS);
    let mut out = std::io::stdout().lock();
    for case in &cases {
        let metadata = case.metadata()?;
        let mut line = case.judge(&client.evaluate_tweets(case.request(), &metadata).await);
        if line.call_failed() {
            line = case.judge(&client.evaluate_tweets(case.request(), &metadata).await);
        }
        serde_json::to_writer(&mut out, &line)?;
        writeln!(out)?;
    }
    Ok(())
}
