mod author_rules;
pub mod context;
#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod golden_corpus;
pub mod metrics;
pub mod registry;
mod rule_spec;
mod tweet_rules;

pub use context::RuleContext;
pub use registry::{RuleEngine, SafetyLevel};

#[cfg(test)]
pub(crate) fn test_context<'a>(
    viewer: &'a crate::models::ViewerFeatures,
    candidate: &'a crate::models::HydratedTweetCandidate,
) -> RuleContext<'a> {
    use std::sync::LazyLock;

    static NSFW_GATING_COUNTRIES: LazyLock<crate::params::NsfwGatingCountries> =
        LazyLock::new(crate::params::NsfwGatingCountries::starting_at_default);
    RuleContext::new(viewer, candidate, &NSFW_GATING_COUNTRIES)
}
