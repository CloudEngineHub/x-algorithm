use crate::hydration::store::KeyOrigin;
use crate::hydration::{Hydrator, Hydrators};
use crate::metrics::Rpc;
use crate::models::{AuthorId, PureCore, TweetFeatures, TweetId};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, strum::IntoStaticStr, strum::VariantArray)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum Relation {
    RetweetSource,
    ConversationRoot,
    ReactedTo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Related {
    pub(super) tweet_id: TweetId,
    pub(super) author: Option<AuthorId>,
}

pub(super) struct RelationSpec {
    pub(super) trigger: Trigger,
    pub(super) role: Role,
}

#[derive(Clone, Copy)]
pub(super) enum Trigger {
    PureCore(fn(&PureCore) -> Option<Related>),
    Tweet(fn(&TweetFeatures) -> Option<Related>),
    Key(KeyOrigin),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    Judged { rpc: Rpc },
    Read,
}

impl Relation {
    pub(super) const fn spec(self) -> RelationSpec {
        match self {
            Relation::RetweetSource => RelationSpec {
                trigger: Trigger::PureCore(retweet_source),
                role: Role::Judged {
                    rpc: Rpc::EvaluateTweets,
                },
            },
            Relation::ConversationRoot => RelationSpec {
                trigger: Trigger::Key(KeyOrigin::ConversationRootToFetch),
                role: Role::Read,
            },
            Relation::ReactedTo => RelationSpec {
                trigger: Trigger::Tweet(reacted_to),
                role: Role::Read,
            },
        }
    }
}

impl Relation {
    pub(super) fn is_on(self, posts_judged_for: Option<Rpc>, planned: Hydrators) -> bool {
        match self.spec().role {
            Role::Judged { rpc } => posts_judged_for == Some(rpc),
            Role::Read => planned
                .iter()
                .any(|node| node.spec().relation == Some(self)),
        }
    }
}

impl Trigger {
    pub(super) const fn input(self) -> Option<Hydrator> {
        match self {
            Trigger::PureCore(_) => Some(Hydrator::PureCore),
            Trigger::Tweet(_) => Some(Hydrator::Tweet),
            Trigger::Key(origin) => origin.input(),
        }
    }

    pub(super) const fn reads(self) -> Hydrators {
        match self {
            Trigger::PureCore(_) => Hydrators::of(Hydrator::PureCore),
            Trigger::Tweet(_) => Hydrators::of(Hydrator::PureCore).with(Hydrator::Tweet),
            Trigger::Key(origin) => origin.reads(),
        }
    }

    pub(super) const fn needs_viewer(self) -> bool {
        match self {
            Trigger::PureCore(_) | Trigger::Tweet(_) => false,
            Trigger::Key(origin) => origin.needs_viewer(),
        }
    }
}

fn retweet_source(core: &PureCore) -> Option<Related> {
    Some(Related {
        tweet_id: core.source_tweet_id?,
        author: core.source_author_id,
    })
}

fn reacted_to(tweet: &TweetFeatures) -> Option<Related> {
    let context = tweet.reaction_context?;
    Some(Related {
        tweet_id: TweetId(context.reacted_to_post_id),
        author: Some(AuthorId(context.reacted_to_user_id)),
    })
}

impl fmt::Display for Relation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spec = self.spec();
        let reads: Vec<&str> = spec.trigger.reads().iter().map(<&str>::from).collect();
        write!(
            f,
            "expand {} after: {}",
            <&str>::from(self),
            reads.join(",")
        )?;
        match spec.role {
            Role::Judged { rpc } => write!(f, " role: judged ({})", <&str>::from(rpc))?,
            Role::Read => f.write_str(" role: read")?,
        }
        if spec.trigger.needs_viewer() {
            f.write_str(" (skipped logged out)")?;
        }
        Ok(())
    }
}
