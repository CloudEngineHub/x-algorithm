use xai_safety_label_store::mval_safety_label_map::decode_mval;
use xai_x_thrift::servo_repo::{CachedValue, CachedValueStatus};

use super::types::{FallbackReason, TwemcacheOutcome};
use super::{codec, proto};

const DECODE_FAILED: TwemcacheOutcome = TwemcacheOutcome::FallThrough(FallbackReason::Decode);

pub(crate) fn decode(bytes: &[u8]) -> TwemcacheOutcome {
    codec::contain_decode_panic(|| decode_inner(bytes)).unwrap_or(DECODE_FAILED)
}

fn decode_inner(bytes: &[u8]) -> TwemcacheOutcome {
    let cv: CachedValue = match xai_x_thrift::deserialize_binary(bytes) {
        Ok(cv) => cv,
        Err(_) => return DECODE_FAILED,
    };
    match cv.status {
        Some(s) if s == CachedValueStatus::NOT_FOUND || s == CachedValueStatus::DELETED => {
            TwemcacheOutcome::NotFound
        }
        Some(s) if s == CachedValueStatus::FOUND => match cv.value.as_deref() {
            Some(value) => match decode_mval(value) {
                Some(labels) => TwemcacheOutcome::Hit(proto::label_map_to_proto(labels)),
                None => DECODE_FAILED,
            },
            None => DECODE_FAILED,
        },
        _ => TwemcacheOutcome::Miss,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use thrift::protocol::{TBinaryOutputProtocol, TSerializable};
    use xai_visibility_filtering_proto as vf_pb;
    use xai_x_thrift::tweet_safety_label::SafetyLabelType;

    fn serialize_thrift<T: TSerializable>(val: &T) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut proto = TBinaryOutputProtocol::new(&mut buf, false);
        val.write_to_out_protocol(&mut proto).unwrap();
        buf
    }

    fn cached_value_blob(status: CachedValueStatus, inner: Option<&[u8]>) -> Vec<u8> {
        let cv = CachedValue::new(
            inner.map(|b| b.to_vec()),
            Some(status),
            None::<i64>,
            None::<i64>,
            None::<i64>,
            None::<i64>,
            None::<i16>,
        );
        serialize_thrift(&cv)
    }

    pub(crate) fn cached_value_not_found() -> Vec<u8> {
        cached_value_blob(CachedValueStatus::NOT_FOUND, None)
    }

    pub(crate) fn cached_value_deleted() -> Vec<u8> {
        cached_value_blob(CachedValueStatus::DELETED, None)
    }

    pub(crate) fn cached_value_do_not_cache() -> Vec<u8> {
        cached_value_blob(CachedValueStatus::DO_NOT_CACHE, None)
    }

    pub(crate) fn cached_value_found_mval() -> Vec<u8> {
        let mval_blob: &[u8] = &[
            0x0c, 0x00, 0x03, 0x0c, 0x5f, 0xff, 0x0d, 0x69, 0x14, 0x0c, 0x0c, 0x00, 0x00, 0x00,
            0x01, 0x01, 0xe7, 0x7c, 0x00, 0x00,
        ];
        cached_value_blob(CachedValueStatus::FOUND, Some(mval_blob))
    }

    #[test]
    fn decode_status_selects_cache_disposition() {
        for (blob, expected) in [
            (cached_value_not_found(), TwemcacheOutcome::NotFound),
            (cached_value_deleted(), TwemcacheOutcome::NotFound),
            (cached_value_do_not_cache(), TwemcacheOutcome::Miss),
        ] {
            assert_eq!(decode(&blob), expected);
        }
    }

    #[test]
    fn decode_found_with_undecodable_mval_falls_through() {
        let blob = cached_value_blob(CachedValueStatus::FOUND, Some(&[0xff; 8]));
        assert_eq!(decode(&blob), DECODE_FAILED);
    }

    #[test]
    fn decode_found_with_no_value_falls_through() {
        let blob = cached_value_blob(CachedValueStatus::FOUND, None);
        assert_eq!(decode(&blob), DECODE_FAILED);
    }

    #[test]
    fn decode_found_inner_matches_expected_proto_fixture() {
        let blob = cached_value_found_mval();
        let expected = vf_pb::SafetyLabelMap {
            labels: std::collections::HashMap::from([(
                i32::from(SafetyLabelType::NSFA_HIGH_PRECISION),
                vf_pb::SafetyLabel::default(),
            )]),
        };

        assert_eq!(decode(&blob), TwemcacheOutcome::Hit(expected));
    }
}
