//! Kaspa names for the creator dashboard: .kas (KNS) and .k (dotk.name).
//!
//! Names live on mainnet. A testnet member is shown by the name of any mainnet wallet they
//! linked, or of the mainnet address for the same key. Lookups are cached for 15 minutes and are
//! only ever used for display: who holds a pass is always decided by the chain, never by a name.
//!
//! The name a person marked primary always wins. If a name service doesn't answer, nothing is
//! guessed: the last names we knew stay up (or none), and we ask again in a minute.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

const TTL: Duration = Duration::from_secs(900);
/// A wallet without names is asked again sooner, so a newly registered name shows up quickly.
const EMPTY_TTL: Duration = Duration::from_secs(600);
/// A lookup that failed (a name service was down or busy) is tried again this soon.
const RETRY_TTL: Duration = Duration::from_secs(60);

/// A name service didn't answer properly (network error, timeout, 5xx or 429).
#[derive(Debug)]
struct Down;

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
pub struct Names {
    /// e.g. "alice.kas"
    pub kas: Option<String>,
    /// e.g. "alice.k"
    pub k: Option<String>,
}

pub struct Resolver {
    http: reqwest::Client,
    kns: String,
    dotk: String,
    cache: Mutex<HashMap<String, (Names, Instant, Duration)>>,
}

impl Default for Resolver {
    fn default() -> Self {
        Resolver {
            http: reqwest::Client::builder().timeout(Duration::from_secs(6)).build().unwrap_or_default(),
            kns: crate::test_switch("KASPASS_KNS_API").unwrap_or_else(|| "https://api.knsdomains.org/mainnet".into()),
            dotk: crate::test_switch("KASPASS_DOTK_API").unwrap_or_else(|| "https://api.dotk.name/v1".into()),
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl Resolver {
    async fn get(&self, url: &str) -> Option<Value> {
        self.fetch(url).await.ok().flatten()
    }

    /// Ok(Some) = an answer, Ok(None) = "not found" (any 4xx but 429), Err = the service is down.
    async fn fetch(&self, url: &str) -> Result<Option<Value>, Down> {
        let r = self.http.get(url).send().await.map_err(|_| Down)?;
        let st = r.status();
        if st.is_server_error() || st.as_u16() == 429 {
            return Err(Down);
        }
        if !st.is_success() {
            return Ok(None);
        }
        r.json().await.map(Some).map_err(|_| Down)
    }

    /// The .kas name to show: the owner's primary name, else their shortest domain. If the
    /// primary-name lookup fails, it doesn't fall back (that would show a name they didn't pick).
    async fn kas_of(&self, address: &str) -> Result<Option<String>, Down> {
        let primary = self.fetch(&format!("{}/api/v1/primary-name/{address}", self.kns)).await?;
        if let Some(n) = primary.as_ref().and_then(|v| v["data"]["domain"]["fullName"].as_str()) {
            return Ok(Some(n.to_string()));
        }
        let Some(v) = self.fetch(&format!("{}/api/v1/assets?owner={address}&type=domain&pageSize=50", self.kns)).await? else { return Ok(None) };
        let Some(list) = v["data"]["assets"].as_array().or(v["assets"].as_array()) else { return Ok(None) };
        Ok(shortest(list.iter().filter_map(|a| a["asset"].as_str()).filter(|n| n.ends_with(".kas"))))
    }

    /// The .k name to show: the one marked primary, else the shortest (then alphabetical), as dotk advises.
    async fn k_of(&self, address: &str) -> Result<Option<String>, Down> {
        let Some(v) = self.fetch(&format!("{}/addresses/{address}", self.dotk)).await? else { return Ok(None) };
        let primary = v["cards"].as_array().and_then(|c| c.iter().find(|c| c["records"]["primary"] == Value::Bool(true))).and_then(|c| c["name"].as_str());
        let name = match primary {
            Some(n) => Some(n.to_string()),
            None => v["names"].as_array().and_then(|a| shortest(a.iter().filter_map(|n| n.as_str()))),
        };
        Ok(name.map(|name| if name.ends_with(".k") { name } else { format!("{name}.k") }))
    }

    /// The mainnet address that owns a typed name: "alice.kas" (KNS) or "alice.k" (dotk).
    /// Nothing is guessed: an unknown or unsupported name is an error the person sees.
    pub async fn address_of(&self, input: &str) -> Result<(String, String), String> {
        let name = input.trim().to_lowercase();
        if let Some(bare) = name.strip_suffix(".kas") {
            if bare.is_empty() || bare.contains('.') || !bare.chars().all(|c| c.is_alphanumeric() || c == '-') {
                return Err(format!("\"{name}\" isn't a .kas name"));
            }
            let v = self.get(&format!("{}/api/v1/{name}/owner", self.kns)).await.ok_or_else(|| format!("{name} isn't registered, or KNS didn't answer"))?;
            let owner = v["data"]["owner"].as_str().ok_or_else(|| format!("{name} has no owner on KNS"))?;
            return Ok((name.clone(), owner.to_string()));
        }
        if let Some(bare) = name.strip_suffix(".k") {
            if bare.contains('.') {
                return Err("subnames (like pay.alice.k) aren't supported yet; use the main name".into());
            }
            if bare.is_empty() || !bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(format!("\"{name}\" isn't a .k name"));
            }
            let v = self.get(&format!("{}/names/{bare}", self.dotk)).await.ok_or_else(|| format!("{name} isn't registered, or dotk didn't answer"))?;
            let addr = v["address"].as_str().ok_or_else(|| format!("{name} is held by a contract, so it has no address to send to"))?;
            return Ok((name.clone(), addr.to_string()));
        }
        Err("enter a Kaspa address, or a name ending in .kas or .k".into())
    }

    /// Names for one mainnet address (cached).
    pub async fn lookup(&self, address: &str) -> Names {
        if !address.starts_with("kaspa:") {
            return Names::default();
        }
        let before = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(address).cloned();
        if let Some((n, t, ttl)) = &before {
            if t.elapsed() < *ttl {
                return n.clone();
            }
        }
        let (kas, k) = tokio::join!(self.kas_of(address), self.k_of(address));
        let old = before.map(|(n, _, _)| n).unwrap_or_default();
        let failed = kas.is_err() || k.is_err();
        // A service that's down keeps the last name we knew from it, never a guess.
        let n = Names { kas: kas.unwrap_or(old.kas), k: k.unwrap_or(old.k) };
        let ttl = if failed { RETRY_TTL } else if n == Names::default() { EMPTY_TTL } else { TTL };
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(address.to_string(), (n.clone(), Instant::now(), ttl));
        n
    }

    /// Names for a member, from the first of their wallets (in order) that has one of each.
    pub async fn for_member(&self, wallets: &[String]) -> Names {
        let mut out = Names::default();
        for w in wallets.iter().filter(|w| w.starts_with("kaspa:")) {
            let n = self.lookup(w).await;
            out.kas = out.kas.or(n.kas);
            out.k = out.k.or(n.k);
            if out.kas.is_some() && out.k.is_some() {
                break;
            }
        }
        out
    }
}

fn shortest<'a>(it: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut v: Vec<&str> = it.collect();
    v.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
    v.first().map(|s| s.to_string())
}

/// The address for the same key on another network.
pub fn twin_on(address: &str, prefix: kaspa_addresses::Prefix) -> Option<String> {
    let a: kaspa_addresses::Address = address.try_into().ok()?;
    Some(kaspa_addresses::Address::new(prefix, a.version, &a.payload).to_string())
}

/// The mainnet address for the same key as a testnet address (or the address itself).
pub fn mainnet_twin(address: &str) -> Option<String> {
    let a: kaspa_addresses::Address = address.try_into().ok()?;
    Some(kaspa_addresses::Address::new(kaspa_addresses::Prefix::Mainnet, a.version, &a.payload).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, extract::Path, http::StatusCode, routing::get};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A fake KNS + dotk. While `down` is set, KNS's primary-name lookup answers 503.
    async fn fake(down: Arc<AtomicBool>) -> String {
        let d = down.clone();
        let app = Router::new()
            .route(
                "/kns/api/v1/primary-name/:a",
                get(move |Path(_a): Path<String>| {
                    let d = d.clone();
                    async move {
                        if d.load(Ordering::SeqCst) {
                            (StatusCode::SERVICE_UNAVAILABLE, String::new())
                        } else {
                            (StatusCode::OK, r#"{"success":true,"data":{"domain":{"fullName":"sample.kas"}}}"#.to_string())
                        }
                    }
                }),
            )
            .route("/kns/api/v1/assets", get(|| async { r#"{"data":{"assets":[{"asset":"gamer.kas"},{"asset":"66.kas"},{"asset":"sample.kas"}]}}"# }))
            .route("/dotk/addresses/:a", get(|| async { r#"{"names":["zz","sample"],"cards":[{"name":"sample","records":{"primary":true}}]}"# }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        base
    }

    fn resolver(base: &str) -> Resolver {
        Resolver { http: reqwest::Client::new(), kns: format!("{base}/kns"), dotk: format!("{base}/dotk"), cache: Mutex::new(HashMap::new()) }
    }

    #[tokio::test]
    async fn primary_wins_and_a_down_service_is_not_a_guess() {
        let down = Arc::new(AtomicBool::new(true));
        let r = resolver(&fake(down.clone()).await);
        let a = "kaspa:qexamplememberaddressforthenamestest";
        // KNS down from the start: no .kas name rather than the shortest (66.kas); .k still primary.
        let n = r.lookup(a).await;
        assert_eq!(n.kas, None);
        assert_eq!(n.k.as_deref(), Some("sample.k"));
        // It's asked again soon (not cached for 15 minutes), and then the primary shows.
        down.store(false, Ordering::SeqCst);
        r.cache.lock().unwrap_or_else(|e| e.into_inner()).get_mut(a).unwrap().1 -= RETRY_TTL;
        assert_eq!(r.lookup(a).await.kas.as_deref(), Some("sample.kas"));
        // Down again later: the last known name stays up.
        down.store(true, Ordering::SeqCst);
        r.cache.lock().unwrap_or_else(|e| e.into_inner()).get_mut(a).unwrap().1 -= TTL;
        assert_eq!(r.lookup(a).await.kas.as_deref(), Some("sample.kas"));
    }
}
