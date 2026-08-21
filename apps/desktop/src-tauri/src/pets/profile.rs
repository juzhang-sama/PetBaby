use crate::creation::name::normalize_display_name;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PetGender {
    Unknown,
    Male,
    Female,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PetProfile {
    pub schema_version: u32,
    pub pet_id: String,
    pub display_name: String,
    pub gender: PetGender,
    pub birth_date: Option<String>,
    pub editable: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PetProfileUpdate {
    pub display_name: String,
    pub gender: PetGender,
    pub birth_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DateParts {
    year: u16,
    month: u16,
    day: u16,
}

pub fn validate_profile_update(update: PetProfileUpdate) -> Result<PetProfileUpdate, String> {
    validate_profile_update_at(update, today_local())
}

fn validate_profile_update_at(
    mut update: PetProfileUpdate,
    today: DateParts,
) -> Result<PetProfileUpdate, String> {
    update.display_name = normalize_display_name(&update.display_name)?;
    if let Some(value) = update.birth_date.as_deref() {
        validate_birth_date_at(value, today)?;
    }
    Ok(update)
}

pub(crate) fn validate_birth_date(value: &str) -> Result<(), String> {
    validate_birth_date_at(value, today_local())
}

fn validate_birth_date_at(value: &str, today: DateParts) -> Result<(), String> {
    let date = parse_gregorian_date(value)?;
    if date > today {
        return Err("birth date cannot be in the future".into());
    }
    Ok(())
}

fn parse_gregorian_date(value: &str) -> Result<DateParts, String> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return Err("birth date must use YYYY-MM-DD".into());
    }

    let year = parse_digits(&bytes[0..4]);
    let month = parse_digits(&bytes[5..7]);
    let day = parse_digits(&bytes[8..10]);
    if year == 0 || !(1..=12).contains(&month) {
        return Err("birth date is not a valid Gregorian date".into());
    }
    let max_day = match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day == 0 || day > max_day {
        return Err("birth date is not a valid Gregorian date".into());
    }
    Ok(DateParts { year, month, day })
}

fn parse_digits(bytes: &[u8]) -> u16 {
    bytes
        .iter()
        .fold(0, |value, byte| value * 10 + u16::from(byte - b'0'))
}

fn is_leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(windows)]
fn today_local() -> DateParts {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(system_time: *mut SystemTime);
    }

    let mut time = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    // SAFETY: GetLocalTime writes one fully initialized SYSTEMTIME to a valid pointer.
    unsafe { GetLocalTime(&mut time) };
    DateParts {
        year: time.year,
        month: time.month,
        day: time.day,
    }
}

#[cfg(not(windows))]
fn today_local() -> DateParts {
    use std::time::{SystemTime, UNIX_EPOCH};

    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_secs() / 86_400) as i64)
        .unwrap_or(0);
    civil_date_from_unix_days(days)
}

#[cfg(not(windows))]
fn civil_date_from_unix_days(days: i64) -> DateParts {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    DateParts {
        year: year as u16,
        month: month as u16,
        day: day as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(name: &str, birth_date: Option<&str>) -> PetProfileUpdate {
        PetProfileUpdate {
            display_name: name.into(),
            gender: PetGender::Female,
            birth_date: birth_date.map(str::to_owned),
        }
    }

    fn today() -> DateParts {
        DateParts {
            year: 2025,
            month: 3,
            day: 8,
        }
    }

    #[test]
    fn profile_serializes_with_camel_case_contract() {
        let profile = PetProfile {
            schema_version: 1,
            pet_id: "pet-a".into(),
            display_name: "米米".into(),
            gender: PetGender::Unknown,
            birth_date: None,
            editable: true,
            updated_at: "123".into(),
        };

        assert_eq!(
            serde_json::to_value(profile).unwrap(),
            json!({
                "schemaVersion": 1,
                "petId": "pet-a",
                "displayName": "米米",
                "gender": "unknown",
                "birthDate": null,
                "editable": true,
                "updatedAt": "123"
            })
        );
    }

    #[test]
    fn update_deserialization_rejects_unknown_fields_and_invalid_gender() {
        assert!(serde_json::from_value::<PetProfileUpdate>(json!({
            "displayName": "米米",
            "gender": "female",
            "birthDate": null,
            "extra": true
        }))
        .is_err());
        assert!(serde_json::from_value::<PetProfileUpdate>(json!({
            "displayName": "米米",
            "gender": "other",
            "birthDate": null
        }))
        .is_err());
    }

    #[test]
    fn validation_reuses_display_name_normalization_rules() {
        assert_eq!(
            validate_profile_update_at(update("  米米  ", None), today())
                .unwrap()
                .display_name,
            "米米"
        );
        assert!(validate_profile_update_at(update("   ", None), today()).is_err());
        assert!(validate_profile_update_at(update(&"猫".repeat(21), None), today()).is_err());
        assert!(validate_profile_update_at(update("米\u{0000}米", None), today()).is_err());
        assert!(validate_profile_update_at(update("米\u{2028}米", None), today()).is_err());
    }

    #[test]
    fn validates_gregorian_leap_year_and_century_rules() {
        for date in ["2000-02-29", "2024-02-29", "1900-02-28"] {
            assert!(validate_profile_update_at(update("米米", Some(date)), today()).is_ok());
        }
        for date in ["1900-02-29", "2100-02-29", "2025-02-29", "0000-01-01"] {
            assert!(
                validate_profile_update_at(
                    update("米米", Some(date)),
                    DateParts {
                        year: 2200,
                        month: 1,
                        day: 1,
                    },
                )
                .is_err(),
                "{date} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_non_ascii_or_non_fixed_width_date_formats() {
        for date in [
            "２０２４-０２-２９",
            "2024-2-29",
            "2024/02/29",
            "2024-02-29x",
            " 2024-02-29",
        ] {
            assert!(
                validate_profile_update_at(update("米米", Some(date)), today()).is_err(),
                "{date} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_today_and_rejects_future_birth_dates() {
        assert!(validate_profile_update_at(update("米米", Some("2025-03-08")), today()).is_ok());
        assert!(validate_profile_update_at(update("米米", Some("2025-03-09")), today()).is_err());
        assert!(validate_profile_update_at(update("米米", Some("2026-01-01")), today()).is_err());
    }
}
