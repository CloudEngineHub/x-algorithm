use super::{RequestRow, Store};
use crate::hydration::fetcher::AnyFetcher;
use crate::hydration::hydrator::Edge;
use crate::hydration::inventory::Answer;
use crate::hydration::{Cause, Hydrator};
use crate::models::{AuthorId, ReplyTo, TweetId};
use xai_core_entities::entities::{ConversationControl, ConversationControlArm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConversationRootAuthor {
    PureCore(AuthorId),
    ConversationControl(AuthorId),
    RootPost(TweetId),
    RootPureCoreFailed,
    Unresolved,
}

impl Store {
    pub(super) fn conversation_root_author(
        &self,
        row: &RequestRow,
    ) -> Option<ConversationRootAuthor> {
        let id = row.tweet_id()?.0;
        let (conversation_id, parent_id) = match self.pure_cores.request().get(id)?.reply_to? {
            ReplyTo::Root { root_author_id, .. } => {
                return Some(ConversationRootAuthor::PureCore(root_author_id));
            }
            ReplyTo::Nested {
                conversation_id,
                parent_id,
            } => (conversation_id, parent_id),
        };
        if let Some(author) = self.root_author_in_batch(conversation_id, parent_id) {
            return Some(ConversationRootAuthor::PureCore(author));
        }
        let root = match (self.controls.get(id), self.controls.answer(id)) {
            (Some(control), _) => ConversationRootAuthor::ConversationControl(AuthorId(
                control.conversation_tweet_author_id,
            )),
            (None, Answer::NotFound) if !self.requested_tweet_ids.contains(&conversation_id) => {
                ConversationRootAuthor::RootPost(conversation_id)
            }
            (None, Answer::NotFound)
                if self.existence_miss(Hydrator::PureCore, conversation_id.0)
                    == Some(Cause::Failed) =>
            {
                ConversationRootAuthor::RootPureCoreFailed
            }
            (None, _) => ConversationRootAuthor::Unresolved,
        };
        Some(root)
    }

    fn root_author_in_batch(
        &self,
        conversation_id: TweetId,
        parent_id: TweetId,
    ) -> Option<AuthorId> {
        let row_core = |id: TweetId| {
            self.pure_cores
                .request()
                .get(id.0)
                .filter(|_| self.requested_tweet_ids.contains(&id))
        };
        row_core(conversation_id)
            .map(|root| root.author_id)
            .or_else(|| match row_core(parent_id)?.reply_to? {
                ReplyTo::Root {
                    conversation_id: parent_conversation_id,
                    root_author_id,
                } => (parent_conversation_id == conversation_id).then_some(root_author_id),
                ReplyTo::Nested { .. } => None,
            })
    }

    pub(super) fn root_not_following_viewer(&self, control: &ConversationControl) -> Option<u64> {
        if control.arm != ConversationControlArm::MyNetwork {
            return None;
        }
        let root = control.conversation_tweet_author_id;
        let holds = *self.edge_fetcher(Edge::FollowedBy)?.get(root)?;
        (!holds).then_some(root)
    }
}
