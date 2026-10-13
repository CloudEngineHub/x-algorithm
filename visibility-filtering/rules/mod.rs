mod author_rules;
mod context;
#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod golden_corpus;
mod levels;
pub mod metrics;
pub mod registry;
mod rule_spec;
mod safety_level;
mod tweet_rules;

#[cfg(test)]
use crate::models::{PostCandidate, ViewerFeatures};
#[cfg(test)]
use crate::params::CountryLists;
use context::RuleContext;
pub(crate) use registry::ExplainedFact;
pub use registry::RuleEngine;
pub(crate) use rule_spec::Truth;
pub use safety_level::SafetyLevel;

#[cfg(test)]
pub(crate) fn test_context<'a>(
    viewer: &'a ViewerFeatures,
    candidate: &'a PostCandidate,
) -> RuleContext<'a> {
    use std::sync::LazyLock;

    static COUNTRY_LISTS: LazyLock<CountryLists> = LazyLock::new(CountryLists::starting_at_default);
    RuleContext::new(viewer, candidate, &COUNTRY_LISTS)
}
