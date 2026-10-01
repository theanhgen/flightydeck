//! HTTPS client for api.flightyapp.com. Only https, only that host, no redirects,
//! rate-limited writes. Logs endpoints (never the token) to stderr.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::blocking::Response;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT,
};
use url::Url;
use zeroize::Zeroizing;

use crate::creds;
use crate::ctx::Ctx;
use crate::error::{Error, Result};
use crate::proto;

pub const HOST: &str = "api.flightyapp.com";
const TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_GAP: Duration = Duration::from_secs(2);
const RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// When the last write call went out, across all clients in this process.
static LAST_WRITE: Mutex<Option<Instant>> = Mutex::new(None);

/// Transport policy: https, exactly `api.flightyapp.com`, default port, no userinfo.
pub fn url_allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(HOST)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

pub struct Client {
    http: reqwest::blocking::Client,
    ctx: Ctx,
    base: Url,
    write_gap: Duration,
    backoff: Duration,
    /// The only origin a unit-test client may call (instead of the real host).
    #[cfg(test)]
    test_origin: Option<url::Origin>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Credentials are read from `ctx` fresh for every request, never cached.
    pub fn new(ctx: &Ctx) -> Result<Client> {
        let base = Url::parse(&format!("https://{HOST}/")).expect("static URL");
        Self::build(ctx, base, WRITE_GAP, RETRY_BACKOFF)
    }

    fn build(ctx: &Ctx, base: Url, write_gap: Duration, backoff: Duration) -> Result<Client> {
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .build()
            .map_err(|e| Error::Other(format!("HTTP client setup failed: {e}")))?;
        Ok(Client {
            http,
            ctx: ctx.clone(),
            base,
            write_gap,
            backoff,
            #[cfg(test)]
            test_origin: None,
        })
    }

    /// Client aimed at a local mock (`http://127.0.0.1:<port>`), with short delays.
    #[cfg(test)]
    pub(crate) fn for_test(ctx: &Ctx, base: &str, write_gap: Duration) -> Client {
        let base = Url::parse(base).unwrap();
        let mut c = Self::build(ctx, base.clone(), write_gap, Duration::from_millis(10)).unwrap();
        c.test_origin = Some(base.origin());
        c
    }

    fn check_url(&self, url: &Url) -> Result<()> {
        // A test client reaches only its mock, never the real host.
        #[cfg(test)]
        let allowed = match &self.test_origin {
            Some(origin) => *origin == url.origin(),
            None => url_allowed(url),
        };
        #[cfg(not(test))]
        let allowed = url_allowed(url);
        if allowed {
            Ok(())
        } else {
            Err(Error::Refused(format!(
                "refusing to send credentials to a URL outside https://{HOST}"
            )))
        }
    }

    fn headers(&self) -> Result<HeaderMap> {
        let c = creds::load(&self.ctx)?;
        let bad = |what: &str| Error::NotReady(format!("Flighty {what} has unexpected characters"));
        let bearer = Zeroizing::new(format!("Bearer {}", c.token.expose()));
        let mut auth = HeaderValue::from_str(&bearer).map_err(|_| bad("sign-in token"))?;
        auth.set_sensitive(true);
        let mut build = HeaderValue::from_str(&c.build_token).map_err(|_| bad("build token"))?;
        build.set_sensitive(true);
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, auth);
        h.insert("x-flighty-build-token", build);
        h.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-protobuf"),
        );
        h.insert(ACCEPT, HeaderValue::from_static("application/x-protobuf"));
        h.insert(
            USER_AGENT,
            HeaderValue::from_str(&c.user_agent).map_err(|_| bad("app version"))?,
        );
        h.insert("x-flighty-locale", HeaderValue::from_static("en_US"));
        Ok(h)
    }

    /// One POST. The URL is checked before credentials are even loaded.
    fn post(&self, url: &Url, body: Vec<u8>) -> Result<Response> {
        self.check_url(url)?;
        let mut headers = self.headers()?;
        if body.is_empty() {
            headers.insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
        }
        let resp = self
            .http
            .post(url.clone())
            .headers(headers)
            .body(body)
            .send()
            .map_err(|e| {
                eprintln!("POST {} -> failed", url.path());
                // `without_url`: the sync URL carries a cursor that shouldn't end up in messages.
                let e = e.without_url();
                if e.is_timeout() {
                    Error::Api("Flighty API timed out".into())
                } else {
                    Error::Api(format!("Flighty API unreachable: {e}"))
                }
            })?;
        eprintln!("POST {} -> {}", url.path(), resp.status().as_u16());
        Ok(resp)
    }

    fn write_post(&self, url: &Url, body: Vec<u8>) -> Result<Response> {
        self.check_url(url)?;
        let mut last = LAST_WRITE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = *last {
            let wait = self.write_gap.saturating_sub(at.elapsed());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        let resp = self.post(url, body);
        *last = Some(Instant::now());
        resp
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        self.base
            .join(path)
            .map_err(|e| Error::Other(format!("bad API path: {e}")))
    }

    /// Search one airline + number + local date. `Ok(None)` = no such flight.
    /// Retries once on 429/5xx.
    pub fn search(&self, airline_id: &str, number: &str, date: &str) -> Result<Option<String>> {
        let url = self.endpoint("/v1/search")?;
        let body = proto::search_request(airline_id, number, date);
        let mut resp = self.post(&url, body.clone())?;
        if retryable(resp.status()) {
            std::thread::sleep(self.backoff);
            resp = self.post(&url, body)?;
        }
        let bytes = ok_body(resp, "search")?;
        proto::flight_uuid(&bytes)
    }

    /// Add a flight to the account: as passenger (`add`) or not (`follow`).
    pub fn subscribe(&self, flight_id: &str, passenger: bool) -> Result<()> {
        if !proto::is_uuid(flight_id) {
            return Err(Error::BadInput(format!(
                "not a Flighty flight id: {flight_id:?}"
            )));
        }
        let mut url = self.endpoint(&format!("/v1/flight/{flight_id}/subscribe"))?;
        url.set_query(Some(if passenger {
            "is_passenger=true&source"
        } else {
            "is_passenger=false&source"
        }));
        ok_body(self.write_post(&url, Vec::new())?, "subscribe").map(drop)
    }

    /// EXPERIMENTAL: delete a flight via the sync endpoint. The `randomSeq` semantics are unverified.
    pub fn remove(&self, sync_url: &str, flight_id: &str) -> Result<()> {
        if !proto::is_uuid(flight_id) {
            return Err(Error::BadInput(format!(
                "not a Flighty flight id: {flight_id:?}"
            )));
        }
        let mut url = Url::parse(sync_url)
            .map_err(|_| Error::NotReady("Flighty's stored sync URL is not a valid URL".into()))?;
        url.query_pairs_mut()
            .append_pair("fast_flight_sync", "true");
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        let seq = rand::random_range(0..1_000_000_000u64);
        let body = proto::remove_request(flight_id, now, seq);
        ok_body(self.write_post(&url, body)?, "remove").map(drop)
    }
}

fn retryable(s: StatusCode) -> bool {
    s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error()
}

/// The body of a 2xx response, else `Error::Api` naming the status (never the body).
fn ok_body(resp: Response, what: &str) -> Result<Vec<u8>> {
    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        return Err(Error::Api(match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => format!(
                "Flighty {what} failed (HTTP {code}): sign-in rejected; open the Flighty app to refresh it"
            ),
            _ => format!("Flighty {what} failed (HTTP {code})"),
        }));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| {
        Error::Api(format!(
            "Flighty {what}: reading the response failed: {}",
            e.without_url()
        ))
    })
}

#[cfg(test)]
pub(crate) mod mock {
    //! A tiny HTTP/1.1 server on 127.0.0.1 that records requests and plays scripted responses.

    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    pub struct Request {
        pub method: String,
        /// Path and query.
        pub target: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Request {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    pub struct Mock {
        pub base: String,
        requests: Arc<Mutex<Vec<Request>>>,
    }

    impl Mock {
        /// Serve `responses` (status, body) in order; 404 once they run out.
        pub fn start(responses: Vec<(u16, Vec<u8>)>) -> Mock {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let log = requests.clone();
            let mut queue = VecDeque::from(responses);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let Some(req) = read_request(&mut BufReader::new(&stream)) else {
                        continue;
                    };
                    log.lock().unwrap().push(req);
                    let (status, body) = queue.pop_front().unwrap_or((404, Vec::new()));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
            });
            Mock { base, requests }
        }

        pub fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn read_request(r: &mut impl BufRead) -> Option<Request> {
        let mut line = String::new();
        r.read_line(&mut line).ok()?;
        let mut parts = line.split_whitespace();
        let method = parts.next()?.to_string();
        let target = parts.next()?.to_string();
        let mut headers = Vec::new();
        loop {
            let mut h = String::new();
            r.read_line(&mut h).ok()?;
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            let (k, v) = h.split_once(':')?;
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
        let len = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0; len];
        r.read_exact(&mut body).ok()?;
        Some(Request {
            method,
            target,
            headers,
            body,
        })
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A `Ctx` over a throwaway DB + Info.plist with a fake (but well-formed) token.

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use crate::ctx::Ctx;

    pub const OWNER: &str = "00000000-0000-4000-8000-000000000001";
    pub const FAKE_BUILD_TOKEN: &str = "fake-build-token";

    pub fn jwt(exp: i64) -> String {
        let payload = format!(r#"{{"sub":"{OWNER}","exp":{exp}}}"#);
        format!(
            "e30.{}.ZmFrZS1zaWduYXR1cmU",
            URL_SAFE_NO_PAD.encode(payload)
        )
    }

    /// Keep the returned dir alive for as long as the `Ctx` is used.
    /// Loads `tests/fixtures` (schema + seeds) into a temp DB, the same data the
    /// integration tests use.
    pub fn ctx() -> (tempfile::TempDir, Ctx) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("MainFlightyDatabase.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        let mut files: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "sql"))
            .collect();
        // schema.sql sorts after seed_*; run it first.
        files.sort_by_key(|p| (!p.ends_with("schema.sql"), p.clone()));
        for f in files {
            conn.execute_batch(&std::fs::read_to_string(&f).unwrap())
                .unwrap();
        }
        conn.execute(
            "UPDATE Account SET authToken = ?1 WHERE rawKind = 'main'",
            [jwt(4102444800)],
        )
        .unwrap();
        drop(conn);

        let plist = dir.path().join("Info.plist");
        let mut d = plist::Dictionary::new();
        d.insert("FlightyBuildToken".into(), FAKE_BUILD_TOKEN.into());
        d.insert("CFBundleShortVersionString".into(), "9.9.9".into());
        d.insert("CFBundleVersion".into(), "999".into());
        plist::Value::Dictionary(d).to_file_xml(&plist).unwrap();

        let mut ctx = Ctx::with_db(&db);
        ctx.app_plist = plist;
        (dir, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::mock::Mock;
    use super::testing;
    use super::*;

    const FLIGHT: &str = "11111111-2222-4333-8444-555555555555";

    fn search_response(uuid: &str) -> Vec<u8> {
        let mut leaf = Vec::new();
        proto::put_bytes_field(&mut leaf, 1, uuid.as_bytes());
        let mut result = Vec::new();
        proto::put_bytes_field(&mut result, 1, &leaf);
        let mut msg = Vec::new();
        proto::put_bytes_field(&mut msg, 2, &result);
        msg
    }

    #[test]
    fn policy() {
        let ok = |s: &str| url_allowed(&Url::parse(s).unwrap());
        assert!(ok("https://api.flightyapp.com/v1/search"));
        assert!(ok("https://api.flightyapp.com:443/v1/sync/full?cursor=x"));
        assert!(!ok("http://api.flightyapp.com/v1/search"));
        assert!(!ok("https://evil.example/v1/search"));
        assert!(!ok("https://api.flightyapp.com.evil.example/"));
        assert!(!ok("https://user:pw@api.flightyapp.com/"));
        assert!(!ok("https://user@api.flightyapp.com/"));
        assert!(!ok("https://api.flightyapp.com:8443/"));
        assert!(!ok("http://127.0.0.1:1234/"));
    }

    #[test]
    fn search_sends_headers_and_decodes_path() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, search_response(FLIGHT))]);
        let c = Client::for_test(&ctx, &mock.base, Duration::ZERO);
        let got = c
            .search("b0000000-0000-4000-8000-000000000001", "111", "2031-03-17")
            .unwrap();
        assert_eq!(got.as_deref(), Some(FLIGHT));

        let reqs = mock.requests();
        assert_eq!(reqs.len(), 1);
        let r = &reqs[0];
        assert_eq!(
            (r.method.as_str(), r.target.as_str()),
            ("POST", "/v1/search")
        );
        assert_eq!(
            r.header("authorization"),
            Some(format!("Bearer {}", testing::jwt(4102444800)).as_str())
        );
        assert_eq!(
            r.header("x-flighty-build-token"),
            Some(testing::FAKE_BUILD_TOKEN)
        );
        assert_eq!(r.header("content-type"), Some("application/x-protobuf"));
        assert_eq!(r.header("accept"), Some("application/x-protobuf"));
        assert_eq!(
            r.header("user-agent"),
            Some("Flighty 9.9.9 (999) com.flightyapp.flighty")
        );
        assert_eq!(r.header("x-flighty-locale"), Some("en_US"));
        assert_eq!(
            r.body,
            proto::search_request("b0000000-0000-4000-8000-000000000001", "111", "2031-03-17")
        );
    }

    #[test]
    fn search_retries_once_on_5xx_and_429() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(503, vec![]), (200, search_response(FLIGHT))]);
        let c = Client::for_test(&ctx, &mock.base, Duration::ZERO);
        assert_eq!(
            c.search("a", "1", "2031-01-01").unwrap().as_deref(),
            Some(FLIGHT)
        );
        assert_eq!(mock.requests().len(), 2);

        let mock = Mock::start(vec![(429, vec![]), (500, vec![])]);
        let c = Client::for_test(&ctx, &mock.base, Duration::ZERO);
        let e = c.search("a", "1", "2031-01-01").unwrap_err();
        assert!(
            matches!(e, Error::Api(ref m) if m.contains("HTTP 500")),
            "{e}"
        );
        assert_eq!(mock.requests().len(), 2);
    }

    #[test]
    fn writes_do_not_retry_and_map_status() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(500, vec![])]);
        let c = Client::for_test(&ctx, &mock.base, Duration::ZERO);
        let e = c.subscribe(FLIGHT, true).unwrap_err();
        assert!(
            matches!(e, Error::Api(ref m) if m.contains("HTTP 500")),
            "{e}"
        );
        assert_eq!(mock.requests().len(), 1);
    }

    #[test]
    fn subscribe_request_shape() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, vec![]), (200, vec![])]);
        let c = Client::for_test(&ctx, &mock.base, Duration::ZERO);
        c.subscribe(FLIGHT, true).unwrap();
        c.subscribe(FLIGHT, false).unwrap();
        let reqs = mock.requests();
        assert_eq!(
            reqs[0].target,
            format!("/v1/flight/{FLIGHT}/subscribe?is_passenger=true&source")
        );
        assert_eq!(
            reqs[1].target,
            format!("/v1/flight/{FLIGHT}/subscribe?is_passenger=false&source")
        );
        assert_eq!(reqs[0].header("content-length"), Some("0"));
        assert!(reqs[0].body.is_empty());
        assert!(c.subscribe("../../x", true).is_err());
        assert_eq!(mock.requests().len(), 2);
    }

    #[test]
    fn writes_are_spaced() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, vec![]), (200, vec![])]);
        let c = Client::for_test(&ctx, &mock.base, Duration::from_millis(300));
        let t = Instant::now();
        c.subscribe(FLIGHT, true).unwrap();
        c.subscribe(FLIGHT, false).unwrap();
        assert!(t.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn remove_appends_param_and_encodes_body() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, vec![])]);
        let c = Client::for_test(&ctx, &mock.base, Duration::ZERO);
        c.remove(&format!("{}v1/sync/full?cursor=abc", mock.base), FLIGHT)
            .unwrap();
        let r = &mock.requests()[0];
        assert_eq!(r.target, "/v1/sync/full?cursor=abc&fast_flight_sync=true");
        // {1: {1: {…}, 11: {1: uuid}}}
        assert_eq!(
            proto::field_path(&r.body, &[1, 11, 1]),
            Some(FLIGHT.as_bytes())
        );
        assert!(proto::field_path(&r.body, &[1, 1]).is_some());
    }

    #[test]
    fn rejected_url_never_gets_credentials() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![]);
        let other = Mock::start(vec![(200, vec![])]);
        let c = Client::for_test(&ctx, &mock.base, Duration::ZERO);
        for url in [
            format!("{}v1/sync/full?cursor=x", other.base), // other port on the same host
            "http://api.flightyapp.com/v1/sync/full?c=1".into(),
            "https://evil.example/v1/sync/full?c=1".into(),
            "https://u:p@api.flightyapp.com/v1/sync/full?c=1".into(),
            "https://api.flightyapp.com:8443/v1/sync/full?c=1".into(),
        ] {
            let e = c.remove(&url, FLIGHT).unwrap_err();
            assert!(matches!(e, Error::Refused(_)), "{url}: {e}");
        }
        assert!(other.requests().is_empty());
        assert!(mock.requests().is_empty());
    }

    #[test]
    fn token_never_in_errors_or_debug() {
        let (_d, ctx) = testing::ctx();
        let token = testing::jwt(4102444800);
        let mock = Mock::start(vec![
            (401, token.clone().into_bytes()),
            (500, token.clone().into_bytes()),
        ]);
        let c = Client::for_test(&ctx, &mock.base, Duration::ZERO);
        let errors = [
            c.subscribe(FLIGHT, true).unwrap_err(),
            c.subscribe(FLIGHT, true).unwrap_err(),
            c.remove("https://evil.example/x", FLIGHT).unwrap_err(),
        ];
        for e in &errors {
            assert!(!format!("{e} {e:?}").contains(&token));
        }
        assert!(errors[0].to_string().contains("HTTP 401"));
        assert!(!format!("{c:?}").contains(&token));
        let creds = creds::load(&ctx).unwrap();
        assert!(!format!("{creds:?}").contains(&token));
    }
}
