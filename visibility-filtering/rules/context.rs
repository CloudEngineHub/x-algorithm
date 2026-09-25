use crate::hydration::Hydrator;
#[cfg(test)]
use crate::hydration::Hydrators;
use crate::models::{
    tweet_timestamp_ms, AuthorFeatures, AuthorLabelSet, HydratedTweetCandidate, SafetyLabelMap,
    TweetFeatures, Viewer, ViewerFeatures, ViewerProfile,
};
use crate::params::NsfwGatingCountries;
use xai_core_entities::entities::ConversationControl;

pub(crate) struct RuleContext<'a> {
    facts: CoreFacts<'a>,
    #[cfg(test)]
    hydrated: Hydrators,
}

#[derive(Clone, Copy)]
pub(super) struct CoreFacts<'a> {
    viewer: &'a ViewerFeatures,
    candidate: &'a HydratedTweetCandidate,
    nsfw_gating_countries: &'a NsfwGatingCountries,
}

impl<'a> RuleContext<'a> {
    pub(super) fn new(
        viewer: &'a ViewerFeatures,
        candidate: &'a HydratedTweetCandidate,
        nsfw_gating_countries: &'a NsfwGatingCountries,
    ) -> Self {
        Self {
            facts: CoreFacts {
                viewer,
                candidate,
                nsfw_gating_countries,
            },
            #[cfg(test)]
            hydrated: Hydrators::all(),
        }
    }

    #[cfg(test)]
    pub(super) fn hydrated_by(self, hydrated: Hydrators) -> Self {
        Self { hydrated, ..self }
    }

    #[inline]
    fn reads(&self, hydrator: Hydrator) -> &'a HydratedTweetCandidate {
        #[cfg(test)]
        assert!(
            self.hydrated.contains(hydrator),
            "a rule reads {hydrator:?}, which its policy does not derive"
        );
        #[cfg(not(test))]
        let _ = hydrator;
        self.facts.candidate
    }

    #[inline]
    pub(super) fn facts(&self) -> CoreFacts<'a> {
        self.facts
    }

    #[inline]
    pub(super) fn tweet_features(&self) -> &'a TweetFeatures {
        &self.reads(Hydrator::Tweet).tweet_features
    }

    #[inline]
    pub(super) fn tweet_safety_labels(&self) -> &'a SafetyLabelMap {
        &self.reads(Hydrator::TweetSafetyLabels).safety_labels
    }

    #[inline]
    pub(super) fn author_features(&self) -> &'a AuthorFeatures {
        &self.reads(Hydrator::AuthorSafety).author_features
    }

    #[inline]
    pub(super) fn author_labels(&self) -> AuthorLabelSet {
        self.reads(Hydrator::AuthorLabels).author_labels
    }

    #[inline]
    pub(super) fn viewer_follows_author(&self) -> bool {
        self.reads(Hydrator::Follows)
            .relationship
            .viewer_follows_author
    }

    #[inline]
    pub(super) fn viewer_blocks_author(&self) -> bool {
        self.reads(Hydrator::Blocks)
            .relationship
            .viewer_blocks_author
    }

    #[inline]
    pub(super) fn viewer_mutes_author(&self) -> bool {
        self.reads(Hydrator::Mutes).relationship.viewer_mutes_author
    }

    #[inline]
    pub(super) fn viewer_mutes_retweets_from_author(&self) -> bool {
        self.reads(Hydrator::MuteRetweets)
            .relationship
            .viewer_mutes_retweets_from_author
    }

    #[inline]
    pub(super) fn blocked_by_author(&self) -> bool {
        self.reads(Hydrator::BlockedByAuthor).blocked_by.author
    }

    #[inline]
    pub(super) fn blocked_by_reply_root_author(&self) -> bool {
        self.reads(Hydrator::BlockedByReplyRoot)
            .blocked_by
            .root_author
    }

    #[inline]
    pub(super) fn viewer_super_follows_exclusive_author(&self) -> bool {
        self.reads(Hydrator::SuperFollowsExclusive)
            .viewer_super_follows_exclusive_author
    }

    #[inline]
    pub(super) fn conversation_control(&self) -> Option<&'a ConversationControl> {
        let features = self
            .reads(Hydrator::ConversationControl)
            .conversation_control
            .as_ref();
        features.map(|features| &features.control)
    }

    #[inline]
    pub(super) fn root_author_follows_viewer(&self) -> Option<bool> {
        let features = self
            .reads(Hydrator::RootFollowsViewer)
            .conversation_control
            .as_ref();
        features.and_then(|features| features.root_author_follows_viewer)
    }

    #[inline]
    pub(super) fn viewer_super_follows_root_author(&self) -> Option<bool> {
        let features = self
            .reads(Hydrator::SuperFollowsRoot)
            .conversation_control
            .as_ref();
        features.and_then(|features| features.viewer_super_follows_root_author)
    }

    #[inline]
    pub(super) fn viewer_country(&self) -> Option<&'a str> {
        let features = self
            .reads(Hydrator::ViewerCountry)
            .conversation_control
            .as_ref();
        features.and_then(|features| features.viewer_country.as_deref())
    }

    #[inline]
    pub(super) fn viewer_profile(&self) -> Option<&'a ViewerProfile> {
        self.reads(Hydrator::ViewerProfile);
        match &self.facts.viewer.viewer {
            Viewer::LoggedIn { profile, .. } => Some(profile),
            Viewer::LoggedOut => None,
        }
    }
}

impl<'a> CoreFacts<'a> {
    #[inline]
    pub(super) fn tweet_id(self) -> u64 {
        self.candidate.tweet_id
    }

    #[inline]
    pub(super) fn viewer_id(self) -> Option<u64> {
        self.viewer.viewer.user_id()
    }

    #[inline]
    pub(super) fn request_country(self) -> Option<&'a str> {
        self.viewer.country_code.as_deref()
    }

    #[inline]
    pub(super) fn is_author_viewer(self) -> bool {
        self.viewer_id() == Some(self.candidate.author_id)
    }

    #[inline]
    pub(super) fn created_after(self, unix_ms: u64) -> bool {
        tweet_timestamp_ms(self.candidate.tweet_id) > unix_ms
    }

    #[inline]
    pub(super) fn nsfw_gating_country(self, country_code: &str) -> bool {
        self.nsfw_gating_countries.contains(country_code)
    }
}
