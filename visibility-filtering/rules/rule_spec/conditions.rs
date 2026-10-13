use super::*;

const fn any_of<const N: usize>(leaves: &'static [Predicate; N]) -> Cond {
    const {
        assert!(
            N >= 2,
            "a disjunction needs two or more leaves; write the leaf"
        )
    };
    Cond(Condition::Any(leaves))
}

#[expect(clippy::indexing_slicing, reason = "every index stays below N")]
const fn leaves<const N: usize>(branches: [Cond; N]) -> [Predicate; N] {
    let mut leaves = [branches[0].leaf(); N];
    let mut index = 1;
    while index < N {
        leaves[index] = branches[index].leaf();
        index += 1;
    }
    leaves
}

macro_rules! disjunctions {
    ($($(#[$attr:meta])* $name:ident = $branches:expr;)+) => {
        $(
            $(#[$attr])*
            pub(in crate::rules) const fn $name() -> Cond {
                any_of(&const { leaves($branches) })
            }
        )+

        #[cfg(test)]
        pub(in crate::rules) const NAMED: &[(&str, Cond)] = &[$((stringify!($name), $name())),+];
    };
}

disjunctions! {
    has_nsfw_media_label = [
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_has_label(SafetyLabelType::NSFW_HIGH_RECALL),
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
    ];
    is_nsfw_flagged = [
        author_is_nsfw_user(),
        author_is_nsfw_admin(),
        tweet_nsfw_user_flag(),
        tweet_nsfw_admin_flag(),
    ];
    has_nsfw_text_or_card_label = [
        tweet_has_label(SafetyLabelType::NSFW_TEXT),
        tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE),
    ];
    is_nsfw_admin = [author_is_nsfw_admin(), tweet_nsfw_admin_flag()];
    is_nsfw_user = [author_is_nsfw_user(), tweet_nsfw_user_flag()];
        tweet_is_nsfw_flagged = [tweet_nsfw_admin_flag(), tweet_nsfw_user_flag()];
    is_nsfw_sensitive_tweet = [
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_has_label(SafetyLabelType::NSFW_HIGH_RECALL),
        tweet_has_label(SafetyLabelType::NSFW_TEXT),
        tweet_has_label(SafetyLabelType::NSFW_TEXT_HIGH_PRECISION),
        tweet_has_label(SafetyLabelType::NSFW_VIDEO),
        tweet_nsfw_admin_flag(),
        tweet_nsfw_user_flag(),
    ];
    is_nsfw_sensitive_author = [
        author_has_label(AuthorLabel::NsfwAvatarImage),
        author_has_label(AuthorLabel::NsfwBannerImage),
        author_has_label(AuthorLabel::NsfwHighPrecision),
        author_has_label(AuthorLabel::NsfwHighRecall),
        author_has_label(AuthorLabel::NsfwNearPerfect),
        author_is_nsfw_admin(),
        author_is_nsfw_user(),
    ];
    has_fosnr_label = [
        tweet_has_label(SafetyLabelType::FOSNR_HATEFUL_CONDUCT),
        tweet_has_label(SafetyLabelType::FOSNR_VIOLENT_SPEECH),
        tweet_has_label(SafetyLabelType::FOSNR_ABUSE),
        tweet_has_label(SafetyLabelType::FOSNR_CIVIC_INTEGRITY),
        tweet_has_label(SafetyLabelType::FOSNR_ABUSE_INSULTS),
    ];
}

#[cfg(test)]
pub(in crate::rules) mod for_tests {
    use super::*;

    pub(in crate::rules) fn any(branches: &[Cond]) -> Cond {
        assert!(branches.len() >= 2);
        let leaves = branches.iter().map(|branch| branch.leaf()).collect();
        Cond(Condition::Any(Vec::leak(leaves)))
    }

    pub(in crate::rules) const fn viewer_does_not_follow_author() -> Cond {
        Cond(Condition::Not(Predicate::ViewerFollowsAuthor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_disjunction_is_named_once() {
        let leaf_set = |condition: Cond| {
            let Condition::Any(set) = condition.0 else {
                unreachable!("NAMED lists disjunctions");
            };
            let mut leaves: Vec<String> = set.iter().map(|leaf| format!("{leaf:?}")).collect();
            leaves.sort();
            leaves
        };
        for (index, &(name, condition)) in NAMED.iter().enumerate() {
            for &(other, other_condition) in &NAMED[..index] {
                assert_ne!(
                    leaf_set(condition),
                    leaf_set(other_condition),
                    "{other}() and {name}() list the same leaves; keep one"
                );
            }
        }
    }
}
