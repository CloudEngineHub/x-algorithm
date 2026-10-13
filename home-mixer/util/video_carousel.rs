use crate::models::candidate::{CandidateHelpers, PostCandidate};
use crate::models::query::{RequestType, ScoredPostsQuery};
use crate::params::{EnableVideoCarousel, VIDEO_CAROUSEL_MAX_VIDEOS, VIDEO_CAROUSEL_MIN_VIDEOS};
use std::collections::HashSet;

const MAX_VERTICAL_ASPECT_RATIO: f32 = 1.0;

pub(crate) fn is_enabled(query: &ScoredPostsQuery) -> bool {
    query.request_type == RequestType::ForYou
        && query.params.get(EnableVideoCarousel)
        && query.video_carousel_eligible
}

pub(crate) fn select_videos<'a>(
    candidates: impl IntoIterator<Item = &'a PostCandidate>,
    dedupe_by_author: bool,
    semantic_id_dedupe_levels: usize,
) -> Vec<PostCandidate> {
    let mut videos: Vec<&PostCandidate> = candidates
        .into_iter()
        .filter(|candidate| is_carousel_video(candidate) && is_vertical(candidate))
        .collect();
    videos.sort_by(|a, b| video_open_score(b).total_cmp(&video_open_score(a)));
    let videos = take_distinct(videos, dedupe_by_author, semantic_id_dedupe_levels);
    if videos.len() < VIDEO_CAROUSEL_MIN_VIDEOS {
        return Vec::new();
    }
    videos.into_iter().cloned().collect()
}

fn take_distinct(
    ranked: Vec<&PostCandidate>,
    dedupe_by_author: bool,
    semantic_id_dedupe_levels: usize,
) -> Vec<&PostCandidate> {
    let mut authors = HashSet::new();
    let mut prefixes: HashSet<&[i32]> = HashSet::new();
    let mut kept = Vec::with_capacity(VIDEO_CAROUSEL_MAX_VIDEOS);
    for video in ranked {
        if kept.len() == VIDEO_CAROUSEL_MAX_VIDEOS {
            break;
        }
        let author = video.get_original_author_id();
        if dedupe_by_author && authors.contains(&author) {
            continue;
        }
        let prefix = video.semantic_id_prefix(semantic_id_dedupe_levels);
        if prefix.is_some_and(|prefix| prefixes.contains(prefix)) {
            continue;
        }
        authors.insert(author);
        prefixes.extend(prefix);
        kept.push(video);
    }
    kept
}

pub(crate) fn take_videos_by_video_open(
    candidates: &mut Vec<PostCandidate>,
    count: usize,
) -> Vec<PostCandidate> {
    let (mut videos, rest): (Vec<_>, Vec<_>) = std::mem::take(candidates)
        .into_iter()
        .partition(is_carousel_video);
    videos.sort_by(|a, b| video_open_score(b).total_cmp(&video_open_score(a)));
    let overflow = videos.split_off(count.min(videos.len()));
    *candidates = rest;
    candidates.extend(overflow);
    for video in &mut videos {
        video.video_carousel_extra = true;
    }
    videos
}

fn is_carousel_video(candidate: &PostCandidate) -> bool {
    candidate.has_video == Some(true)
        && candidate.in_reply_to_tweet_id.is_none()
        && candidate.phoenix_scores.video_open_score.is_some()
}

fn is_vertical(candidate: &PostCandidate) -> bool {
    candidate
        .video_aspect_ratio
        .is_some_and(|ratio| ratio < MAX_VERTICAL_ASPECT_RATIO)
}

fn video_open_score(candidate: &PostCandidate) -> f64 {
    candidate.phoenix_scores.video_open_score.unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::candidate::PhoenixScores;

    fn video(tweet_id: u64, aspect_ratio: f32, video_open: f64) -> PostCandidate {
        PostCandidate {
            tweet_id,
            has_video: Some(true),
            video_aspect_ratio: Some(aspect_ratio),
            phoenix_scores: PhoenixScores {
                video_open_score: Some(video_open),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn tweet_ids(candidates: &[PostCandidate]) -> Vec<u64> {
        candidates
            .iter()
            .map(|candidate| candidate.tweet_id)
            .collect()
    }

    #[test]
    fn keeps_vertical_videos_ranked_by_video_open() {
        let candidates = vec![
            video(1, 0.5625, 0.1),
            video(2, 1.7778, 0.9),
            video(3, 1.0, 0.8),
            video(4, 0.5625, 0.5),
            video(5, 0.75, 0.3),
            PostCandidate {
                in_reply_to_tweet_id: Some(9),
                ..video(6, 0.5625, 0.7)
            },
        ];

        assert_eq!(
            tweet_ids(&select_videos(&candidates, false, 0)),
            vec![4, 5, 1]
        );
    }

    fn vertical(tweet_id: u64, author_id: u64, sid: &[i32], video_open: f64) -> PostCandidate {
        PostCandidate {
            author_id,
            semantic_ids: (!sid.is_empty()).then(|| sid.to_vec()),
            ..video(tweet_id, 0.5625, video_open)
        }
    }

    #[test]
    fn author_dedupe_keeps_the_top_video_per_original_author() {
        let candidates = vec![
            vertical(1, 10, &[], 0.9),
            PostCandidate {
                retweeted_user_id: Some(10),
                ..vertical(2, 20, &[], 0.8)
            },
            vertical(3, 10, &[], 0.7),
            vertical(4, 30, &[], 0.6),
            vertical(5, 40, &[], 0.5),
        ];

        assert_eq!(
            tweet_ids(&select_videos(&candidates, true, 0)),
            vec![1, 4, 5]
        );
    }

    #[test]
    fn semantic_id_dedupe_matches_full_prefixes_only() {
        let candidates = vec![
            vertical(1, 10, &[1, 2, 3], 0.9),
            vertical(2, 20, &[1, 2, 9], 0.8),
            vertical(3, 30, &[1, 5, 3], 0.7),
            vertical(4, 40, &[], 0.6),
            vertical(5, 50, &[1], 0.5),
        ];

        assert_eq!(
            tweet_ids(&select_videos(&candidates, false, 2)),
            vec![1, 3, 4, 5]
        );
    }

    #[test]
    fn dedupe_fills_from_lower_ranks_up_to_the_max() {
        let candidates: Vec<_> = (1..=8)
            .map(|id| {
                vertical(
                    id,
                    if id <= 3 { 10 } else { id },
                    &[],
                    1.0 - id as f64 / 10.0,
                )
            })
            .collect();

        assert_eq!(
            tweet_ids(&select_videos(&candidates, true, 0)),
            vec![1, 4, 5, 6, 7]
        );
    }

    #[test]
    fn dedupe_below_the_minimum_drops_the_carousel() {
        let candidates = vec![
            vertical(1, 10, &[], 0.9),
            vertical(2, 10, &[], 0.8),
            vertical(3, 20, &[], 0.7),
        ];

        assert!(select_videos(&candidates, true, 0).is_empty());
    }
}
