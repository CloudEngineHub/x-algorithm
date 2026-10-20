use super::RequestTweet;
use crate::clients::socialgraph_client::{EdgeDirection, EdgeFrom, EdgeQuery};
use crate::hydration::batch::{HydrationBatch, RawHydrationBatch};
use crate::hydration::decode::author::DecodedAuthor;
use crate::hydration::fetcher::{AnyFetcher, Fetcher};
use crate::hydration::hydrator::{PairEdge, UserPair};
use crate::hydration::{candidate_count_by_key, Hydrator, Hydrators, QuoteEntry, Quoted};
use crate::models::{AuthorId, PairEdges, PureCore, QuoteFacts, QuoteLink, TweetId};
use rustc_hash::FxHashMap;
use std::cell::OnceCell;
use strum::VariantArray;
use xai_core_entities::entities::QuotedTweet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairKey {
    Asked(UserPair),
    Unasked,
    InputFailed,
}

#[derive(Default)]
pub(super) struct Quotes {
    entries: Vec<QuoteEntry>,
    outers_to_fetch: Vec<TweetId>,
    pub(super) quoted_tweets: Fetcher<QuotedTweet>,
    pair_edges: [Fetcher<bool, UserPair>; PairEdge::VARIANTS.len()],
}

pub(super) struct Pairing<'s> {
    request_tweets: &'s [RequestTweet],
    pure_cores: &'s Fetcher<PureCore>,
    authors: &'s Fetcher<DecodedAuthor>,
    author_by_tweet: OnceCell<FxHashMap<TweetId, Option<AuthorId>>>,
}

impl<'s> Pairing<'s> {
    pub(super) fn new(
        request_tweets: &'s [RequestTweet],
        pure_cores: &'s Fetcher<PureCore>,
        authors: &'s Fetcher<DecodedAuthor>,
    ) -> Self {
        Self {
            request_tweets,
            pure_cores,
            authors,
            author_by_tweet: OnceCell::new(),
        }
    }

    fn quoted_author(&self, inner: TweetId) -> Option<AuthorId> {
        let author_by_tweet = self.author_by_tweet.get_or_init(|| {
            let mut author_by_tweet = FxHashMap::default();
            for request_tweet in self.request_tweets {
                author_by_tweet
                    .entry(request_tweet.tweet_id)
                    .or_insert(request_tweet.author);
            }
            author_by_tweet
        });
        author_by_tweet.get(&inner).copied().flatten()
    }

    fn link(&self, quote: QuoteEntry) -> Option<QuoteLink> {
        let core = self.pure_cores.get(quote.outer.0)?;
        Some(QuoteLink {
            outer: quote.outer,
            quoting_author: core.source_author_id.unwrap_or(core.author_id).get(),
        })
    }

    fn key(&self, edge: PairEdge, quote: QuoteEntry) -> PairKey {
        let (Some(quoted), Some(link)) = (self.quoted_author(quote.inner), self.link(quote)) else {
            return PairKey::InputFailed;
        };
        let (quoted, quoting) = (quoted.get(), link.quoting_author);
        if quoted == quoting {
            return PairKey::Unasked;
        }
        match edge {
            PairEdge::AuthorBlocksOuterAuthor => PairKey::Asked(UserPair {
                source: quoted,
                destination: quoting,
            }),
            PairEdge::OuterAuthorFollowsAuthor => match self
                .authors
                .get(quoted)
                .filter(|_| !self.authors.is_incomplete(quoted))
            {
                None => PairKey::InputFailed,
                Some((author, _)) if author.is_protected => PairKey::Asked(UserPair {
                    source: quoting,
                    destination: quoted,
                }),
                Some(_) => PairKey::Unasked,
            },
        }
    }
}

impl Quotes {
    pub(super) fn new(entries: &[QuoteEntry]) -> Self {
        let mut entries = entries.to_vec();
        entries.sort_unstable();
        entries.dedup();
        Self {
            entries,
            ..Self::default()
        }
    }

    pub(super) fn outer_of(&self, inner: TweetId) -> Option<TweetId> {
        self.entries
            .iter()
            .find(|quote| quote.inner == inner)
            .map(|quote| quote.outer)
    }

    pub(super) fn outers_to_fetch(&self) -> &[TweetId] {
        &self.outers_to_fetch
    }

    pub(super) fn join_outers(&mut self, request_tweets: &[RequestTweet]) {
        let mut outers: Vec<TweetId> = self
            .entries
            .iter()
            .map(|quote| quote.outer)
            .filter(|&outer| {
                !request_tweets
                    .iter()
                    .any(|request_tweet| request_tweet.tweet_id == outer)
            })
            .collect();
        outers.sort_unstable();
        outers.dedup();
        self.outers_to_fetch = outers;
    }

    pub(super) fn quoted_author(&self, inner: TweetId) -> Option<AuthorId> {
        self.entries
            .iter()
            .filter(|quote| quote.inner == inner)
            .find_map(|quote| {
                self.quoted_tweets
                    .get(quote.outer.0)
                    .filter(|quoted| quoted.tweet_id == inner.0 && quoted.user_id != 0)
            })
            .map(|quoted| AuthorId(quoted.user_id))
    }

    pub(super) fn is_quoted_author_failed(&self, inner: TweetId) -> bool {
        self.entries
            .iter()
            .filter(|quote| quote.inner == inner)
            .any(|quote| self.quoted_tweets.is_incomplete(quote.outer.0))
    }

    fn asked_pairs(&self, edge: PairEdge, pairing: &Pairing<'_>) -> Vec<UserPair> {
        let mut pairs: Vec<UserPair> = self
            .entries
            .iter()
            .filter_map(|&quote| match pairing.key(edge, quote) {
                PairKey::Asked(pair) => Some(pair),
                PairKey::Unasked | PairKey::InputFailed => None,
            })
            .collect();
        pairs.sort_unstable_by_key(|pair| (pair.source, pair.destination));
        pairs.dedup();
        pairs
    }

    pub(super) fn claim_pairs(
        &mut self,
        nodes: Hydrators,
        pairing: &Pairing<'_>,
    ) -> Vec<EdgeQuery> {
        let mut queries: Vec<EdgeQuery> = Vec::new();
        for edge in nodes.iter().filter_map(Hydrator::pair_edge) {
            let pairs = self.asked_pairs(edge, pairing);
            let Some(fetcher) = self.pair_edges.get_mut(usize::from(edge as u8)) else {
                continue;
            };
            for pair in fetcher.claim(pairs) {
                let from = EdgeFrom::User(pair.source);
                match queries.last_mut() {
                    Some(query) if query.graph == edge.graph() && query.from == from => {
                        query.destination_ids.push(pair.destination);
                    }
                    _ => queries.push(EdgeQuery {
                        graph: edge.graph(),
                        direction: EdgeDirection::Forward,
                        from,
                        destination_ids: vec![pair.destination],
                    }),
                }
            }
        }
        queries
    }

    pub(super) fn land_pairs(
        &mut self,
        queries: &[EdgeQuery],
        answers: Vec<RawHydrationBatch<bool>>,
    ) {
        for (query, answer) in queries.iter().zip(answers) {
            let (Some(edge), EdgeFrom::User(source)) =
                (PairEdge::of_graph(query.graph), query.from)
            else {
                continue;
            };
            let pair = |destination| UserPair {
                source,
                destination,
            };
            let claimed: Vec<UserPair> = query.destination_ids.iter().copied().map(pair).collect();
            let answers = answer
                .into_hydrated()
                .into_iter()
                .map(|(destination, holds)| (pair(destination), holds))
                .collect();
            if let Some(fetcher) = self.pair_edges.get_mut(usize::from(edge as u8)) {
                fetcher.land(&claimed, HydrationBatch::from_hydrated(answers));
            }
        }
    }

    pub(super) fn candidate_count_by_quoted_author(
        &self,
        nodes: Hydrators,
        pairing: &Pairing<'_>,
    ) -> FxHashMap<u64, usize> {
        let quoted = |edge| {
            self.entries
                .iter()
                .filter_map(move |&quote| match pairing.key(edge, quote) {
                    PairKey::Asked(pair) => Some(pair.destination),
                    PairKey::Unasked | PairKey::InputFailed => None,
                })
        };
        candidate_count_by_key(
            nodes
                .iter()
                .filter_map(Hydrator::pair_edge)
                .flat_map(quoted),
        )
    }

    fn pair_fetcher(&self, edge: PairEdge) -> Option<&Fetcher<bool, UserPair>> {
        self.pair_edges.get(usize::from(edge as u8))
    }

    pub(super) fn incomplete(&self, callable: Hydrators) -> Hydrators {
        callable
            .iter()
            .filter(|node| {
                node.pair_edge()
                    .and_then(|edge| self.pair_fetcher(edge))
                    .is_some_and(|fetcher| fetcher.has_incomplete())
            })
            .fold(Hydrators::empty(), Hydrators::with)
    }

    pub(super) fn quoted(
        &self,
        callable: Hydrators,
        pairing: &Pairing<'_>,
    ) -> FxHashMap<QuoteEntry, Quoted> {
        self.entries
            .iter()
            .map(|&quote| (quote, self.quoted_entry(quote, callable, pairing)))
            .collect()
    }

    fn quoted_entry(
        &self,
        quote: QuoteEntry,
        callable: Hydrators,
        pairing: &Pairing<'_>,
    ) -> Quoted {
        let mut pair_edges = PairEdges::default();
        let mut failed = Hydrators::empty();
        for node in callable.iter() {
            let Some((edge, fetcher)) = node
                .pair_edge()
                .and_then(|edge| Some((edge, self.pair_fetcher(edge)?)))
            else {
                continue;
            };
            let holds = match pairing.key(edge, quote) {
                PairKey::Asked(pair) => {
                    if !fetcher.is_claimed(pair) || fetcher.is_incomplete(pair) {
                        failed = failed.with(node);
                    }
                    fetcher.get(pair).copied().unwrap_or_default()
                }
                PairKey::Unasked => false,
                PairKey::InputFailed => {
                    failed = failed.with(node);
                    false
                }
            };
            match edge {
                PairEdge::AuthorBlocksOuterAuthor => pair_edges.author_blocks_outer_author = holds,
                PairEdge::OuterAuthorFollowsAuthor => {
                    pair_edges.outer_author_follows_author = holds
                }
            }
        }
        Quoted {
            facts: QuoteFacts {
                link: pairing.link(quote),
                pair_edges,
            },
            failed,
        }
    }
}
