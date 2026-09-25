use crate::models::{ViewerAge, ViewerProfile};
use xai_core_entities::entities::{AccessPolicy, VerifiedType};
use xai_core_entities::gizmoduck_client::ViewerData;

fn has_verified_badge(data: &ViewerData) -> bool {
    matches!(
        data.verified_type,
        Some(VerifiedType::Business | VerifiedType::Government)
    ) || data.is_blue_verified
}

pub(super) fn viewer_profile(data: ViewerData) -> ViewerProfile {
    let viewer_age = match data.age_in_years {
        Some(age) => ViewerAge::Known(age),
        None if data.user_exists => ViewerAge::NotStated,
        None => ViewerAge::Unknown,
    };
    ViewerProfile {
        allows_sensitive_media: data.nsfw_view.unwrap_or(false),
        viewer_age,
        has_verified_badge: has_verified_badge(&data),
        is_read_only: data.access_policy == AccessPolicy::BounceAllPublicWrites,
        account_country_code: data.account_country_code.map(|c| c.to_ascii_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_existence_and_preference_determine_age_and_sensitive_media() {
        for (data, expected_age, expected_sensitive_media) in [
            (
                ViewerData {
                    user_exists: true,
                    nsfw_view: Some(false),
                    age_in_years: None,
                    ..Default::default()
                },
                ViewerAge::NotStated,
                false,
            ),
            (
                ViewerData {
                    user_exists: true,
                    nsfw_view: Some(true),
                    age_in_years: None,
                    ..Default::default()
                },
                ViewerAge::NotStated,
                true,
            ),
            (ViewerData::default(), ViewerAge::Unknown, false),
        ] {
            let viewer = viewer_profile(data);
            assert_eq!(viewer.viewer_age, expected_age);
            assert_eq!(viewer.allows_sensitive_media, expected_sensitive_media);
        }
    }

    #[test]
    fn verified_badge_requires_org_type_or_blue_check() {
        let cases = [
            (Some(VerifiedType::Business), false, true),
            (Some(VerifiedType::Government), false, true),
            (None, true, true),
            (Some(VerifiedType::User), false, false),
            (Some(VerifiedType::Notable), false, false),
            (None, false, false),
        ];
        for (verified_type, is_blue_verified, expected) in cases {
            let data = ViewerData {
                verified_type,
                is_blue_verified,
                ..Default::default()
            };
            assert_eq!(
                viewer_profile(data).has_verified_badge,
                expected,
                "{verified_type:?} blue={is_blue_verified}"
            );
        }
    }

    #[test]
    fn only_bounce_all_public_writes_makes_the_viewer_read_only() {
        for (access_policy, expected) in [
            (AccessPolicy::Normal, false),
            (AccessPolicy::BounceAll, false),
            (AccessPolicy::BounceAllWritesAndNpci, false),
            (AccessPolicy::BounceAllPublicWrites, true),
            (AccessPolicy::BounceOnUnsuspension, false),
        ] {
            let viewer = viewer_profile(ViewerData {
                access_policy,
                ..Default::default()
            });
            assert_eq!(viewer.is_read_only, expected, "{access_policy:?}");
        }
    }
}
