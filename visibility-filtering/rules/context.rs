use crate::hydration::Hydrator;
#[cfg(test)]
use crate::hydration::Hydrators;
use crate::models::{
    tweet_timestamp_ms, AuthorFeatures, ConversationControlFeatures, ExclusiveContentFeatures,
    HydratedTweetCandidate, SafetyLabelMap, TweetFeatures, Viewer, ViewerAuthorRelationship,
    ViewerFeatures, ViewerProfile,
};
use crate::params::NsfwGatingCountries;

pub(crate) struct RuleContext<'a> {
    viewer: &'a ViewerFeatures,
    candidate: &'a HydratedTweetCandidate,
    nsfw_gating_countries: &'a NsfwGatingCountries,
    #[cfg(test)]
    hydrated: Hydrators,
}

impl<'a> RuleContext<'a> {
    pub(super) fn new(
        viewer: &'a ViewerFeatures,
        candidate: &'a HydratedTweetCandidate,
        nsfw_gating_countries: &'a NsfwGatingCountries,
    ) -> Self {
        Self {
            viewer,
            candidate,
            nsfw_gating_countries,
            #[cfg(test)]
            hydrated: Hydrators::all(),
        }
    }

    #[cfg(test)]
    pub(super) fn hydrated_by(self, hydrated: Hydrators) -> Self {
        Self { hydrated, ..self }
    }

    #[inline]
    fn reads(&self, hydrator: Hydrator) {
        #[cfg(test)]
        assert!(
            self.hydrated.contains(hydrator),
            "a rule reads {hydrator:?}, which its policy does not derive"
        );
        #[cfg(not(test))]
        let _ = hydrator;
    }

    #[inline]
    pub(super) fn tweet_features(&self) -> &'a TweetFeatures {
        self.reads(Hydrator::Tweet);
        &self.candidate.tweet_features
    }

    #[inline]
    pub(super) fn tweet_safety_labels(&self) -> &'a SafetyLabelMap {
        self.reads(Hydrator::TweetSafetyLabels);
        &self.candidate.safety_labels
    }

    #[inline]
    pub(super) fn author_features(&self) -> &'a AuthorFeatures {
        self.reads(Hydrator::Author);
        &self.candidate.author_features
    }

    #[inline]
    pub(super) fn relationship(&self) -> &'a ViewerAuthorRelationship {
        self.reads(Hydrator::Relationship);
        &self.candidate.relationship
    }

    #[inline]
    pub(super) fn exclusive_content(&self) -> Option<&'a ExclusiveContentFeatures> {
        self.reads(Hydrator::ExclusiveContent);
        self.candidate.exclusive_content.as_ref()
    }

    #[inline]
    pub(super) fn conversation_control(&self) -> Option<&'a ConversationControlFeatures> {
        self.reads(Hydrator::ConversationControl);
        self.candidate.conversation_control.as_ref()
    }

    #[inline]
    pub(super) fn viewer_profile(&self) -> Option<&'a ViewerProfile> {
        self.reads(Hydrator::ViewerProfile);
        match &self.viewer.viewer {
            Viewer::LoggedIn { profile, .. } => Some(profile),
            Viewer::LoggedOut => None,
        }
    }

    #[inline]
    pub(super) fn tweet_id(&self) -> u64 {
        self.candidate.tweet_id
    }

    #[inline]
    pub(super) fn viewer_id(&self) -> Option<u64> {
        self.viewer.viewer.user_id()
    }

    #[inline]
    pub(super) fn request_country(&self) -> Option<&'a str> {
        self.viewer.country_code.as_deref()
    }

    #[inline]
    pub(super) fn is_author_viewer(&self) -> bool {
        self.viewer_id() == Some(self.candidate.author_id)
    }

    #[inline]
    pub(super) fn created_after(&self, unix_ms: u64) -> bool {
        tweet_timestamp_ms(self.candidate.tweet_id) > unix_ms
    }

    #[inline]
    pub(super) fn viewer_country(&self) -> Option<&'a str> {
        self.viewer_profile()
            .and_then(|p| p.account_country_code.as_deref())
            .or(self.request_country())
    }

    #[inline]
    pub(super) fn nsfw_gating_country(&self, country_code: &str) -> bool {
        self.nsfw_gating_countries.contains(country_code)
    }
}
