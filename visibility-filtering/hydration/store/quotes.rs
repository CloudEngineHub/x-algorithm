use super::{RequestRow, Store};
use crate::clients::socialgraph_client::{EdgeDirection, EdgeFrom, EdgeQuery};
use crate::hydration::batch::{HydrationBatch, RawHydrationBatch};
use crate::hydration::fetcher::{AnyFetcher, Fetcher};
use crate::hydration::hydrator::{PairEdge, UserPair};
use crate::hydration::inventory::classify;
use crate::hydration::relation::{quoting_author, Related};
use crate::hydration::{candidate_count_by_key, Cause, Hydrator, Hydrators, QuoteEntry, Quoted};
use crate::models::{AuthorId, EdgeSet, Quote, QuoteLink, TweetId};
use rustc_hash::FxHashMap;
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
    pub(super) quoted_tweets: Fetcher<QuotedTweet>,
    pair_edges: [Fetcher<bool, UserPair>; PairEdge::VARIANTS.len()],
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

    pub(super) fn quoted_author(&self, inner: TweetId) -> Result<AuthorId, Cause> {
        let quotes = || {
            self.entries
                .iter()
                .filter(move |quote| quote.inner == inner)
        };
        let named = quotes().find_map(|quote| {
            self.quoted_tweets
                .get(quote.outer.0)
                .filter(|quoted| quoted.tweet_id == inner.0 && quoted.user_id != 0)
        });
        match named {
            Some(quoted) => Ok(AuthorId(quoted.user_id)),
            None if quotes().any(|quote| self.quoted_tweets.is_incomplete(quote.outer.0)) => {
                Err(Cause::Failed)
            }
            None => Err(Cause::NotFound),
        }
    }

    fn pair_fetcher(&self, edge: PairEdge) -> Option<&Fetcher<bool, UserPair>> {
        self.pair_edges.get(usize::from(edge as u8))
    }

    fn pair_fetcher_mut(&mut self, edge: PairEdge) -> Option<&mut Fetcher<bool, UserPair>> {
        self.pair_edges.get_mut(usize::from(edge as u8))
    }

    pub(super) fn land_pairs<'q>(
        &mut self,
        queries: impl Iterator<Item = (PairEdge, &'q EdgeQuery)>,
        answers: Vec<RawHydrationBatch<bool>>,
    ) {
        for ((edge, query), answer) in queries.zip(answers) {
            let EdgeFrom::User(source) = query.from else {
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
            if let Some(fetcher) = self.pair_fetcher_mut(edge) {
                fetcher.land(&claimed, HydrationBatch::from_hydrated(answers));
            }
        }
    }
}

impl Store {
    pub(super) fn quote_outers(&self) -> Vec<(usize, Related)> {
        self.quotes
            .entries
            .iter()
            .filter_map(|quote| {
                let parent = self
                    .rows
                    .iter()
                    .position(|row| row.tweet_id() == Some(quote.inner))?;
                let related = Related {
                    tweet_id: quote.outer,
                    author: None,
                };
                Some((parent, related))
            })
            .collect()
    }

    fn paired_rows(&self) -> impl Iterator<Item = (&RequestRow, TweetId)> {
        self.paired
            .iter()
            .filter_map(|&(parent, outer)| Some((self.rows.get(parent)?, outer)))
    }

    fn quoting_author(&self, outer: TweetId) -> Option<AuthorId> {
        self.pure_cores.request().get(outer.0).map(quoting_author)
    }

    fn pair_key(&self, edge: PairEdge, parent: &RequestRow, outer: TweetId) -> PairKey {
        let (Some(quoted), Some(quoting)) = (parent.author, self.quoting_author(outer)) else {
            return PairKey::InputFailed;
        };
        let (quoted, quoting) = (quoted.get(), quoting.get());
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
                .request()
                .get(quoted)
                .filter(|_| !self.authors.request().is_incomplete(quoted))
            {
                None => PairKey::InputFailed,
                Some(author) if author.features.is_protected => PairKey::Asked(UserPair {
                    source: quoting,
                    destination: quoted,
                }),
                Some(_) => PairKey::Unasked,
            },
        }
    }

    fn asked_pairs(&self, edge: PairEdge) -> Vec<UserPair> {
        let mut pairs: Vec<UserPair> = self
            .paired_rows()
            .filter_map(|(parent, outer)| match self.pair_key(edge, parent, outer) {
                PairKey::Asked(asked) => Some(asked),
                PairKey::Unasked | PairKey::InputFailed => None,
            })
            .collect();
        pairs.sort_unstable_by_key(|pair| (pair.source, pair.destination));
        pairs.dedup();
        pairs
    }

    pub(super) fn claim_pairs(&mut self, nodes: Hydrators) -> Vec<(PairEdge, EdgeQuery)> {
        let asked: Vec<(PairEdge, Vec<UserPair>)> = nodes
            .iter()
            .filter_map(Hydrator::pair_edge)
            .map(|edge| (edge, self.asked_pairs(edge)))
            .collect();
        let mut queries: Vec<(PairEdge, EdgeQuery)> = Vec::new();
        for (edge, pairs) in asked {
            let Some(fetcher) = self.quotes.pair_fetcher_mut(edge) else {
                continue;
            };
            for pair in fetcher.claim(pairs) {
                let from = EdgeFrom::User(pair.source);
                match queries.last_mut() {
                    Some((queried, query)) if *queried == edge && query.from == from => {
                        query.destination_ids.push(pair.destination);
                    }
                    _ => queries.push((
                        edge,
                        EdgeQuery {
                            graph: edge.graph(),
                            direction: EdgeDirection::Forward,
                            from,
                            destination_ids: vec![pair.destination],
                        },
                    )),
                }
            }
        }
        queries
    }

    pub(super) fn candidate_count_by_pair(&self, nodes: Hydrators) -> FxHashMap<u64, usize> {
        let destinations = |edge| {
            self.paired_rows().filter_map(move |(parent, outer)| {
                match self.pair_key(edge, parent, outer) {
                    PairKey::Asked(asked) => Some(asked.destination),
                    PairKey::Unasked | PairKey::InputFailed => None,
                }
            })
        };
        candidate_count_by_key(
            nodes
                .iter()
                .filter_map(Hydrator::pair_edge)
                .flat_map(destinations),
        )
    }

    pub(super) fn quoted(&self) -> FxHashMap<QuoteEntry, Quoted> {
        self.paired_rows()
            .filter_map(|(parent, outer)| {
                let quote = QuoteEntry {
                    inner: parent.tweet_id()?,
                    outer,
                };
                Some((quote, self.quoted_entry(parent, outer)))
            })
            .collect()
    }

    fn quoted_entry(&self, parent: &RequestRow, outer: TweetId) -> Quoted {
        let mut pair_edges = EdgeSet::default();
        let mut failed = Hydrators::empty();
        for node in self.callable.iter() {
            let Some((edge, fetcher)) = node
                .pair_edge()
                .and_then(|edge| Some((edge, self.quotes.pair_fetcher(edge)?)))
            else {
                continue;
            };
            let holds = match self.pair_key(edge, parent, outer) {
                PairKey::Asked(asked) => {
                    if classify(self.stake(node), fetcher.answer(asked)).fails_node() {
                        failed = failed.with(node);
                    }
                    fetcher.get(asked).copied().unwrap_or_default()
                }
                PairKey::Unasked => false,
                PairKey::InputFailed => {
                    failed = failed.with(node);
                    false
                }
            };
            if holds {
                pair_edges = pair_edges.with(node);
            }
        }
        Quoted {
            quote: self.quoting_author(outer).map(|quoting| Quote {
                link: QuoteLink {
                    outer,
                    quoting_author: quoting.get(),
                },
                pair_edges,
            }),
            failed,
        }
    }
}
