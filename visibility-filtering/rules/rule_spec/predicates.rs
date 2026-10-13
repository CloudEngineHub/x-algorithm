use super::*;

#[cfg(test)]
pub(in crate::rules) trait Spelled {
    fn spelled(self) -> String;
}

#[cfg(test)]
impl Spelled for SafetyLabelType {
    fn spelled(self) -> String {
        label_meta(self).map_or_else(|| format!("{self:?}"), |meta| meta.ident.to_owned())
    }
}

#[cfg(test)]
impl Spelled for u64 {
    fn spelled(self) -> String {
        self.to_string()
    }
}

#[cfg(test)]
impl Spelled for &'static str {
    fn spelled(self) -> String {
        self.to_owned()
    }
}

#[cfg(test)]
impl Spelled for AuthorLabel {
    fn spelled(self) -> String {
        format!("{self:?}")
    }
}

#[cfg(test)]
impl Spelled for ConversationControlArm {
    fn spelled(self) -> String {
        format!("{self:?}")
    }
}

#[cfg(test)]
impl Spelled for CountryList {
    fn spelled(self) -> String {
        format!("{self:?}")
    }
}

#[cfg(test)]
impl Spelled for VerifyBlurSupport {
    fn spelled(self) -> String {
        format!("{self:?}")
    }
}

macro_rules! predicates {
    ($(
        $ctor:ident $(($($arg:ident: $ty:ty),*))? = $variant:ident reads $reads:tt
            $(negated as $named:ident)?
            $(negated by audience $audience:ident)?
            => |$facts:pat_param, $value:pat_param| $body:expr
    ),+ $(,)?) => {
        #[derive(Clone, Copy, PartialEq)]
        #[cfg_attr(test, derive(Debug))]
        pub(super) enum Predicate {
            $($variant $(($($ty),*))?,)+
        }

        $(predicates!(
            @constructors $ctor ($($($arg: $ty),*)?) (Predicate::$variant $(($($arg),*))?)
            [$($named)?]
        );)+

        #[cfg(test)]
        pub(super) const CONSTRUCTORS: &[(&str, &str)] = &[$((stringify!($ctor), stringify!($variant))),+];

        impl Predicate {
            pub(super) const fn hydrators(self) -> Hydrators {
                match self {
                    $(Self::$variant { .. } => predicates!(@declare $reads),)+
                }
            }

            pub(super) const fn negation(self) -> Negation {
                match self {
                    $(Self::$variant { .. } => {
                        predicates!(@negation $ctor [$($named)?] [$($audience)?])
                    })+
                }
            }

            #[inline]
            pub(super) fn holds(self, context: &RuleContext<'_>) -> bool {
                match self {
                    $(Self::$variant $(($($arg),*))? => {
                        let $facts = context.facts();
                        let $value = predicates!(@read context $reads);
                        $body
                    })+
                }
            }

            #[cfg(test)]
            pub(super) fn spelled(self, negated: bool) -> String {
                match self {
                    $(Self::$variant $(($($arg),*))? => {
                        let args: &[String] = &[$($($arg.spelled()),*)?];
                        let call = |ctor: &str| {
                            if args.is_empty() {
                                ctor.to_owned()
                            } else {
                                format!("{ctor}({})", args.join(", "))
                            }
                        };
                        match (negated, predicates!(@named $($named)?)) {
                            (false, _) => call(stringify!($ctor)),
                            (true, Some(named)) => call(named),
                            (true, None) => format!("not({})", call(stringify!($ctor))),
                        }
                    })+
                }
            }
        }
    };
    (@named) => { None::<&str> };
    (@named $named:ident) => { Some(stringify!($named)) };
    (@constructors $ctor:ident $params:tt $leaf:tt [$($named:ident)?]) => {
        pub(in crate::rules) const fn $ctor $params -> Cond {
            Cond(Condition::Is $leaf)
        }
        $(
            pub(in crate::rules) const fn $named $params -> Cond {
                Cond(Condition::Not $leaf)
            }
        )?
    };
    (@negation $ctor:ident [] []) => { Negation::Not };
    (@negation $ctor:ident [$named:ident] []) => {
        Negation::Forbidden(concat!(
            "not(", stringify!($ctor), "(..)) has one spelling: write `", stringify!($named), "(..)`"
        ))
    };
    (@negation $ctor:ident [] [$audience:ident]) => {
        Negation::Forbidden(concat!(
            "not(", stringify!($ctor), "()) has one spelling: write `audience: ",
            stringify!($audience), "`"
        ))
    };
    (@declare ()) => { Hydrators::empty() };
    (@declare ($($node:ident),+)) => { Hydrators::empty()$(.with(Hydrator::$node))+ };
    (@declare $node:ident) => { Hydrators::of(Hydrator::$node) };
    (@read $context:ident ()) => { () };
    (@read $context:ident ($($node:ident),+)) => { ($(predicates!(@read $context $node)),+) };
    (@read $context:ident PureCore) => { $context.source_tweet_id() };
    (@read $context:ident Tweet) => { $context.tweet_features() };
    (@read $context:ident ConversationControl) => { $context.conversation_control() };
    (@read $context:ident TweetSafetyLabels) => { $context.tweet_safety_labels() };
    (@read $context:ident ViewerProfile) => { $context.viewer_profile() };
    (@read $context:ident ViewerLabels) => { $context.viewer_has_age_verified_18_label() };
    (@read $context:ident AuthorSafety) => { $context.author_features() };
    (@read $context:ident AuthorLabels) => { $context.author_labels() };
    (@read $context:ident ViewerCountry) => { $context.viewer_country() };
    (@read $context:ident CommunityModeration) => { $context.community_moderation() };
    (@read $context:ident CommunityModerator) => { $context.viewer_is_community_moderator() };
    (@read $context:ident CommunityViewerRemoved) => {
        $context.viewer_is_removed_from_community()
    };
    (@read $context:ident ArticleMediaEntities) => { $context.article_has_media_entities() };
    (@read $context:ident ArticleCoverMedia) => { $context.article_has_cover_media() };
    (@read $context:ident ArticleLifecycle) => { $context.article_lifecycle() };
    (@read $context:ident $edge:ident) => {{
        const { assert!(Hydrator::$edge.is_edge()) };
        $context.edge(Hydrator::$edge)
    }};
    ($($row:tt)*) => {
        compile_error!(
            "every predicates! row reads `constructor(payload: Type) = Variant reads <nodes> \
             [negated as <name>] => |facts, value| body`"
        );
    };
}

predicates! {
    tweet_has_label(label: SafetyLabelType) = TweetHasLabel reads TweetSafetyLabels
        => |_, labels| labels.has_label(label),
    tweet_created_after(unix_ms: u64) = TweetCreatedAfter reads ()
        negated as tweet_created_before
        => |facts, ()| facts.created_after(unix_ms),
    tweet_nsfw_user_flag = TweetNsfwUserFlag reads Tweet
        => |_, tweet| tweet.nsfw.user,
    tweet_nsfw_admin_flag = TweetNsfwAdminFlag reads Tweet
        => |_, tweet| tweet.nsfw.admin,
    tweet_has_media = TweetHasMedia reads (Tweet, ArticleMediaEntities, ArticleCoverMedia)
        => |_, (tweet, body, cover)| tweet.has_media() || body || cover,
    tweet_has_dmca_media = TweetHasDmcaMedia reads Tweet
        => |_, tweet| tweet.has_dmca_media(),
    tweet_is_retweet = TweetIsRetweet reads PureCore
        => |_, source_tweet_id| source_tweet_id.is_some(),
    tweet_is_superseded_edit = TweetIsSupersededEdit reads Tweet
        => |facts, tweet| tweet.is_superseded_edit(facts.tweet_id()),
    tweet_legal_takedown_in_request_country = TweetLegalTakedownInRequestCountry reads Tweet
        => |facts, tweet| tweet.legal_takedown_in(facts.request_country()),
    tweet_local_laws_takedown_in_request_country = TweetLocalLawsTakedownInRequestCountry
        reads Tweet
        => |facts, tweet| tweet.local_laws_takedown_in(facts.request_country()),
    tweet_media_geo_restricted_in_request_country = TweetMediaGeoRestrictedInRequestCountry
        reads Tweet
        => |facts, tweet| tweet.media_restricted_in(facts.request_country()),
    tweet_is_nullcast = TweetIsNullcast reads Tweet
        => |_, tweet| tweet.is_nullcast,
    tweet_is_community_tweet = TweetIsCommunityTweet reads Tweet
        => |_, tweet| tweet.community_id.is_some(),
    tweet_community_tweet_is_hidden = TweetCommunityTweetIsHidden reads CommunityModeration
        => |_, moderation| moderation.is_hidden,
    tweet_community_tweet_author_is_removed = TweetCommunityTweetAuthorIsRemoved
        reads CommunityModeration
        => |_, moderation| moderation.is_author_removed,
    tweet_has_exclusive_content = TweetHasExclusiveContent reads Tweet
        => |_, tweet| tweet.exclusive_conversation_author_id.is_some(),
    tweet_is_trusted_friends_tweet = TweetIsTrustedFriendsTweet reads Tweet
        => |_, tweet| tweet.trusted_friends_list_id.is_some(),
    tweet_has_article = TweetHasArticle reads Tweet
        => |_, tweet| tweet.article_id.is_some(),
    tweet_article_is_published = TweetArticleIsPublished reads ArticleLifecycle
        => |_, lifecycle| lifecycle == Some(ArticleLifecycle::Published),
    tweet_has_conversation_control(arm: ConversationControlArm) = TweetHasConversationControl
        reads ConversationControl
        => |_, control| control.is_some_and(|control| control.arm == arm),
    tweet_has_narrowcast_place = TweetHasNarrowcastPlace reads Tweet
        => |_, tweet| tweet.narrowcast_place_id.is_some(),

    author_has_label(label: AuthorLabel) = AuthorHasLabel reads AuthorLabels
        => |_, labels| labels.has_label(label),
    author_is_suspended = AuthorIsSuspended reads AuthorSafety
        => |_, author| author.is_suspended,
    author_is_deactivated = AuthorIsDeactivated reads AuthorSafety
        => |_, author| author.is_deactivated,
    author_is_erased = AuthorIsErased reads AuthorSafety
        => |_, author| author.is_erased,
    author_is_offboarded = AuthorIsOffboarded reads AuthorSafety
        => |_, author| author.is_offboarded,
    author_is_protected = AuthorIsProtected reads AuthorSafety
        => |_, author| author.is_protected,
    author_is_nsfw_user = AuthorIsNsfwUser reads AuthorSafety
        => |_, author| author.is_nsfw_user,
    author_is_nsfw_admin = AuthorIsNsfwAdmin reads AuthorSafety
        => |_, author| author.is_nsfw_admin,

    viewer_logged_out = ViewerLoggedOut reads ()
        => |facts, ()| facts.viewer_id().is_none(),
    viewer_underage = ViewerUnderage reads ViewerProfile
        => |_, profile| profile.is_some_and(ViewerProfile::is_underage),
    viewer_no_stated_age = ViewerNoStatedAge reads ViewerProfile
        => |_, profile| profile.is_some_and(ViewerProfile::has_no_stated_age),
    viewer_allows_sensitive_media = ViewerAllowsSensitiveMedia reads ViewerProfile
        => |_, profile| profile.is_some_and(|profile| profile.allows_sensitive_media),
    viewer_account_or_request_country_in(list: CountryList) = ViewerAccountOrRequestCountryIn
        reads ViewerProfile
        => |facts, profile| {
            account_or_request_country_in(profile, facts.request_country(), |country| {
                facts.in_country_list(list, country)
            })
        },
    viewer_age_verified = ViewerAgeVerified reads (ViewerProfile, ViewerLabels)
        => |_, (profile, age_verified_18)| {
            age_verified_18 || profile.is_some_and(|profile| profile.has_idv_premium)
        },
    viewer_has_verified_badge = ViewerHasVerifiedBadge reads ViewerProfile
        => |_, profile| profile.is_some_and(|profile| profile.has_verified_badge),
    viewer_read_only = ViewerReadOnly reads ViewerProfile
        => |_, profile| profile.is_some_and(|profile| profile.is_read_only),
    viewer_follows_author = ViewerFollowsAuthor reads Follows
        negated by audience NonFollowers
        => |_, follows| follows,
    viewer_blocks_author = ViewerBlocksAuthor reads Blocks
        => |_, blocks| blocks,
    viewer_mutes_author = ViewerMutesAuthor reads Mutes
        => |_, mutes| mutes,
    viewer_mutes_retweets_from_author = ViewerMutesRetweetsFromAuthor reads MuteRetweets
        => |_, mutes| mutes,
    viewer_is_conversation_author = ViewerIsConversationAuthor reads Tweet
        => |facts, tweet| {
            tweet
                .exclusive_conversation_author_id
                .is_some_and(|author| facts.viewer_id() == Some(author))
        },
    viewer_super_follows_author = ViewerSuperFollowsAuthor reads (Tweet, SuperFollowsExclusive)
        => |_, (tweet, super_follows)| {
            tweet.exclusive_conversation_author_id.is_some() && super_follows
        },
    viewer_is_conversation_root_author = ViewerIsConversationRootAuthor reads ConversationControl
        => |facts, control| {
            control
                .zip(facts.viewer_id())
                .is_some_and(|(control, viewer_id)| {
                    viewer_id == control.conversation_tweet_author_id
                })
        },
    viewer_is_invited_to_conversation = ViewerIsInvitedToConversation reads ConversationControl
        => |facts, control| {
            control
                .zip(facts.viewer_id())
                .is_some_and(|(control, viewer_id)| control.invited_user_ids.contains(&viewer_id))
        },
    viewer_is_followed_by_conversation_root_author = ViewerIsFollowedByConversationRootAuthor
        reads RootFollowsViewer
        => |_, follows| follows,
    viewer_is_in_conversation_root_author_network = ViewerIsInConversationRootAuthorNetwork
        reads (RootFollowsViewer, RootFollowsViewerSecondDegree)
        => |_, (first, second)| first || second,
    viewer_super_follows_conversation_root_author = ViewerSuperFollowsConversationRootAuthor
        reads SuperFollowsRoot
        => |_, super_follows| super_follows,
    viewer_is_blocked_by_author = ViewerIsBlockedByAuthor reads BlockedByAuthor
        => |_, blocked| blocked,
    viewer_is_blocked_by_conversation_root_author = ViewerIsBlockedByConversationRootAuthor
        reads (BlockedByReplyRoot, BlockedByConversationRoot, BlockedByConversationRootPost)
        => |facts, (direct, from_control, fetched)| {
            direct || fetched || (from_control && facts.is_reply())
        },
    viewer_is_in_allowed_country = ViewerIsInAllowedCountry
        reads (ConversationControl, ViewerCountry)
        => |_, (control, country)| {
            control.zip(country).is_some_and(|(control, country)| {
                allows_country(&control.allowed_country_codes, country)
            })
        },
    viewer_is_community_moderator = ViewerIsCommunityModerator reads CommunityModerator
        => |facts, is_moderator| facts.viewer_id().is_some() && is_moderator.unwrap_or(true),
    viewer_is_removed_from_community = ViewerIsRemovedFromCommunity reads CommunityViewerRemoved
        => |_, removed| removed,
    viewer_is_trusted_friends_list_member_or_owner = ViewerIsTrustedFriendsListMemberOrOwner
        reads TrustedFriends
        => |_, holds| holds,
    viewer_is_outside_narrowcast_place = ViewerIsOutsideNarrowcastPlace
        reads OutsideNarrowcastPlace
        => |_, outside| outside,

    request_country_in(list: CountryList) = RequestCountryIn reads ()
        => |facts, ()| {
            facts
                .request_country()
                .is_some_and(|country| facts.in_country_list(list, country))
        },
    request_country_is(country: &'static str) = RequestCountryIs reads ()
        => |facts, ()| facts.request_country() == Some(country),

    client_verify_blur_support_is(support: VerifyBlurSupport) = ClientVerifyBlurSupportIs reads ()
        => |facts, ()| facts.client_capability().verify_blur_support == Some(support),
    client_has_modern_blur = ClientHasModernBlur reads ()
        negated as is_client_deprecated
        => |facts, ()| facts.client_capability().modern_blur,
    client_has_stale_tweet_limits = ClientHasStaleTweetLimits reads ()
        => |facts, ()| facts.client_capability().stale_tweet_limits,
    client_has_community_viewer_removed_limits = ClientHasCommunityViewerRemovedLimits reads ()
        => |facts, ()| facts.client_capability().community_viewer_removed_limits,
    client_blurs_gore_ignoring_settings = ClientBlursGoreIgnoringSettings reads ()
        => |facts, ()| facts.client_capability().gore_blur_ignores_settings,
    client_has_fosnr_rules = ClientHasFosnrRules reads ()
        => |facts, ()| facts.client_capability().fosnr_rules,
    client_needs_fosnr_fallback_drops = ClientNeedsFosnrFallbackDrops reads ()
        => |facts, ()| facts.client_capability().fosnr_fallback_drops,
}
