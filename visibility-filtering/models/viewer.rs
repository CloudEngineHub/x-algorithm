#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Viewer {
    LoggedIn {
        id: u64,
        profile: ViewerProfile,
    },
    #[default]
    LoggedOut,
}

impl Viewer {
    pub fn user_id(&self) -> Option<u64> {
        match self {
            Viewer::LoggedIn { id, .. } => Some(*id),
            Viewer::LoggedOut => None,
        }
    }
}

pub const ADULT_AGE_YEARS: i32 = 18;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewerAge {
    Known(i32),
    NotStated,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewerProfile {
    pub allows_sensitive_media: bool,
    pub viewer_age: ViewerAge,
    pub has_verified_badge: bool,
    pub account_country_code: Option<String>,
}

impl ViewerProfile {
    pub fn is_underage(&self) -> bool {
        matches!(self.viewer_age, ViewerAge::Known(age) if age < ADULT_AGE_YEARS)
    }

    pub fn has_no_stated_age(&self) -> bool {
        self.viewer_age == ViewerAge::NotStated
    }
}

#[derive(Clone, Debug, Default)]
pub struct ViewerFeatures {
    pub viewer: Viewer,
    pub country_code: Option<String>,
}

impl ViewerFeatures {
    pub fn from_request(viewer: Viewer, country_code: Option<String>) -> Self {
        Self {
            viewer,
            country_code: country_code.map(|c| c.to_ascii_lowercase()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stated_age_below_adult_is_underage_and_only_a_missing_age_is_unstated() {
        for (age, underage, no_stated_age) in [
            (ViewerAge::Known(ADULT_AGE_YEARS - 1), true, false),
            (ViewerAge::Known(ADULT_AGE_YEARS), false, false),
            (ViewerAge::NotStated, false, true),
            (ViewerAge::Unknown, false, false),
        ] {
            let profile = ViewerProfile {
                viewer_age: age,
                ..ViewerProfile::default()
            };
            assert_eq!(profile.is_underage(), underage, "{age:?}");
            assert_eq!(profile.has_no_stated_age(), no_stated_age, "{age:?}");
        }
    }
}
