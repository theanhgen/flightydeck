//! Minimal protobuf: hand-written encoder for our few messages + a wire-format walker.

use std::sync::LazyLock;

use regex::Regex;

use crate::error::{Error, Result};

const VARINT: u8 = 0;
const I64: u8 = 1;
const LEN: u8 = 2;
const I32: u8 = 5;

static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap()
});

/// Whether `s` is exactly one UUID (either case).
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36 && UUID.is_match(s)
}

// --- encoder ---

pub(crate) fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v > 0x7f {
        buf.push(0x80 | (v & 0x7f) as u8);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn put_tag(buf: &mut Vec<u8>, field: u32, wire: u8) {
    put_varint(buf, (u64::from(field) << 3) | u64::from(wire));
}

pub(crate) fn put_varint_field(buf: &mut Vec<u8>, field: u32, v: u64) {
    put_tag(buf, field, VARINT);
    put_varint(buf, v);
}

/// A length-delimited field: string, bytes or embedded message.
pub(crate) fn put_bytes_field(buf: &mut Vec<u8>, field: u32, data: &[u8]) {
    put_tag(buf, field, LEN);
    put_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

/// Search request: `{1: {1: airlineUuid, 2: flightNumber}, 3: "YYYY-MM-DD", 4: "FLIGHT_NUMBER"}`.
pub fn search_request(airline_id: &str, number: &str, date: &str) -> Vec<u8> {
    let mut flight = Vec::new();
    put_bytes_field(&mut flight, 1, airline_id.as_bytes());
    put_bytes_field(&mut flight, 2, number.as_bytes());
    let mut msg = Vec::new();
    put_bytes_field(&mut msg, 1, &flight);
    put_bytes_field(&mut msg, 3, date.as_bytes());
    put_bytes_field(&mut msg, 4, b"FLIGHT_NUMBER");
    msg
}

/// Remove request: `{1: {1: {1: unixTs, 2: randomSeq}, 11: {1: flightUuid}}}`.
pub fn remove_request(flight_id: &str, unix_ts: u64, seq: u64) -> Vec<u8> {
    let mut stamp = Vec::new();
    put_varint_field(&mut stamp, 1, unix_ts);
    put_varint_field(&mut stamp, 2, seq);
    let mut deletion = Vec::new();
    put_bytes_field(&mut deletion, 1, flight_id.as_bytes());
    let mut entry = Vec::new();
    put_bytes_field(&mut entry, 1, &stamp);
    put_bytes_field(&mut entry, 11, &deletion);
    let mut msg = Vec::new();
    put_bytes_field(&mut msg, 1, &entry);
    msg
}

// --- walker ---

enum Value<'a> {
    Scalar,
    Len(&'a [u8]),
}

fn take_varint(buf: &mut &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    for i in 0..10 {
        let (&b, rest) = buf.split_first()?;
        *buf = rest;
        let bits = u64::from(b & 0x7f);
        // The 10th byte may only carry the top bit of a u64.
        if i == 9 && bits > 1 {
            return None;
        }
        v |= bits << (7 * i);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn take<'a>(buf: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if buf.len() < n {
        return None;
    }
    let (head, rest) = buf.split_at(n);
    *buf = rest;
    Some(head)
}

/// All fields of one message level, or None if the bytes are not a well-formed message.
fn fields(mut buf: &[u8]) -> Option<Vec<(u64, Value<'_>)>> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let tag = take_varint(&mut buf)?;
        let field = tag >> 3;
        if field == 0 {
            return None;
        }
        let value = match (tag & 7) as u8 {
            VARINT => take_varint(&mut buf).map(|_| Value::Scalar)?,
            I64 => take(&mut buf, 8).map(|_| Value::Scalar)?,
            I32 => take(&mut buf, 4).map(|_| Value::Scalar)?,
            LEN => {
                let n = usize::try_from(take_varint(&mut buf)?).ok()?;
                Value::Len(take(&mut buf, n)?)
            }
            // Groups (3, 4) are deprecated and never used by Flighty; treat as malformed.
            _ => return None,
        };
        out.push((field, value));
    }
    Some(out)
}

/// Follow `path` through nested length-delimited fields (first occurrence at each level)
/// and return the innermost payload. None on a missing field or malformed input.
pub fn field_path<'a>(bytes: &'a [u8], path: &[u32]) -> Option<&'a [u8]> {
    let mut cur = bytes;
    for &want in path {
        cur = fields(cur)?.into_iter().find_map(|(n, v)| match v {
            Value::Len(b) if n == u64::from(want) => Some(b),
            _ => None,
        })?;
    }
    Some(cur)
}

/// Flight UUID from a search response. `Ok(None)` means "no flight found": a well-formed
/// response with no results field (2) and no UUID anywhere. The value at `2 → 1 → 1` is the
/// only UUID ever returned; a byte scan runs only as an early warning of a format change.
pub fn flight_uuid(resp: &[u8]) -> Result<Option<String>> {
    let walked = field_path(resp, &[2, 1, 1])
        .and_then(|b| std::str::from_utf8(b).ok())
        .filter(|s| is_uuid(s))
        .map(str::to_string);
    let scanned = UUID
        .find(&String::from_utf8_lossy(resp))
        .map(|m| m.as_str().to_string());

    if walked.as_deref().map(str::to_ascii_lowercase)
        != scanned.as_deref().map(str::to_ascii_lowercase)
    {
        eprintln!(
            "warning: search response: path 2.1.1 and the UUID scan disagree; Flighty's format may have changed"
        );
    }
    match walked {
        Some(id) => Ok(Some(id)),
        None if fields(resp).is_some() && field_path(resp, &[2]).is_none() && scanned.is_none() => {
            Ok(None)
        }
        None => Err(Error::Api("unexpected search response format".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLIGHT: &str = "11111111-2222-4333-8444-555555555555";
    const DECOY: &str = "99999999-8888-4777-8666-555555555555";

    fn response(flight: &str, decoy_first: bool) -> Vec<u8> {
        let mut leaf = Vec::new();
        put_bytes_field(&mut leaf, 1, flight.as_bytes());
        put_varint_field(&mut leaf, 7, 300);
        let mut result = Vec::new();
        put_varint_field(&mut result, 3, 1);
        put_bytes_field(&mut result, 1, &leaf);
        let mut msg = Vec::new();
        if decoy_first {
            let mut meta = Vec::new();
            put_bytes_field(&mut meta, 1, DECOY.as_bytes());
            put_bytes_field(&mut msg, 1, &meta);
        }
        msg.extend_from_slice(&[0x19, 1, 2, 3, 4, 5, 6, 7, 8]); // field 3, i64
        msg.extend_from_slice(&[0x25, 1, 2, 3, 4]); // field 4, i32
        put_bytes_field(&mut msg, 2, &result);
        msg
    }

    #[test]
    fn varint_encoding() {
        let enc = |v| {
            let mut b = Vec::new();
            put_varint(&mut b, v);
            b
        };
        assert_eq!(enc(0), [0]);
        assert_eq!(enc(1), [1]);
        assert_eq!(enc(300), [0xac, 0x02]);
        assert_eq!(enc(u64::MAX).len(), 10);
        let mut b = enc(u64::MAX);
        assert_eq!(take_varint(&mut b.as_slice()), Some(u64::MAX));
        b[9] = 0x02;
        assert_eq!(take_varint(&mut b.as_slice()), None);
    }

    #[test]
    fn search_request_bytes() {
        let got = search_request("ab", "12", "2031-03-17");
        let mut want = vec![0x0a, 8, 0x0a, 2, b'a', b'b', 0x12, 2, b'1', b'2'];
        want.extend_from_slice(&[0x1a, 10]);
        want.extend_from_slice(b"2031-03-17");
        want.extend_from_slice(&[0x22, 13]);
        want.extend_from_slice(b"FLIGHT_NUMBER");
        assert_eq!(got, want);
    }

    #[test]
    fn remove_request_bytes() {
        let got = remove_request("x", 300, 5);
        // {1: {1: {1: 300, 2: 5}, 11: {1: "x"}}}
        let stamp = [0x08, 0xac, 0x02, 0x10, 0x05];
        let mut entry = vec![0x0a, stamp.len() as u8];
        entry.extend_from_slice(&stamp);
        entry.extend_from_slice(&[0x5a, 3, 0x0a, 1, b'x']);
        let mut want = vec![0x0a, entry.len() as u8];
        want.extend_from_slice(&entry);
        assert_eq!(got, want);
    }

    #[test]
    fn walker_follows_path_not_first_uuid() {
        let resp = response(FLIGHT, true);
        assert_eq!(field_path(&resp, &[2, 1, 1]), Some(FLIGHT.as_bytes()));
        assert_eq!(flight_uuid(&resp).unwrap().as_deref(), Some(FLIGHT));
    }

    #[test]
    fn walker_rejects_malformed_input() {
        let resp = response(FLIGHT, false);
        // Field 2 comes last, so every strict prefix is truncated mid-field or lacks it.
        for cut in 0..resp.len() {
            assert_eq!(field_path(&resp[..cut], &[2, 1, 1]), None, "prefix {cut}");
        }
        assert_eq!(field_path(&[0x0a, 0xff], &[1]), None); // length past end
        assert_eq!(field_path(&[0x0b], &[1]), None); // group wire type
        assert_eq!(field_path(&[0x02, 0x00], &[1]), None); // field number 0
        assert_eq!(
            field_path(
                &[
                    0x0a, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01
                ],
                &[1]
            ),
            None
        );
        assert_eq!(field_path(&[], &[]), Some(&[][..]));
    }

    #[test]
    fn wrong_wire_type_at_path_is_skipped() {
        let mut msg = Vec::new();
        put_varint_field(&mut msg, 2, 7);
        assert_eq!(field_path(&msg, &[2]), None);
    }

    #[test]
    fn empty_response_is_not_found() {
        assert_eq!(flight_uuid(&[]).unwrap(), None);
        let mut msg = Vec::new();
        put_varint_field(&mut msg, 1, 0);
        assert_eq!(flight_uuid(&msg).unwrap(), None);
    }

    #[test]
    fn uuid_elsewhere_fails_closed() {
        // A UUID in the bytes but not at 2.1.1 must never be used.
        let mut meta = Vec::new();
        put_bytes_field(&mut meta, 1, DECOY.as_bytes());
        let mut msg = Vec::new();
        put_bytes_field(&mut msg, 1, &meta);
        assert!(matches!(flight_uuid(&msg), Err(Error::Api(_))));

        // Field 2 present but the leaf is not a UUID.
        let resp = response("not-a-uuid", false);
        assert!(matches!(flight_uuid(&resp), Err(Error::Api(_))));

        // Garbage.
        assert!(matches!(flight_uuid(&[0xff, 0xff]), Err(Error::Api(_))));
    }
}
