use std::collections::hash_map::HashMap;
use std::collections::hash_set::HashSet;
use std::time::Duration;

use libp2p::PeerId;

use utils_log::{info,warn,error};
use utils_metrics::metrics::{MultiGauge, SimpleGauge};

#[cfg(any(not(feature = "wasm"), test))]
use utils_misc::time::native::current_timestamp;

#[cfg(all(feature = "wasm", not(test)))]
use utils_misc::time::wasm::current_timestamp;


#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct NetworkConfig {
    /// Minimum delay will be multiplied by backoff, it will be half the actual minimum value
    min_delay: Duration,
    /// Maximum delay
    max_delay: Duration,
    quality_bad_threshold: f64,
    quality_offline_threshold: f64,
    quality_step: f64,
    ignore_timeframe: Duration,
    backoff_exponent: f64,
    backoff_min: f64,
    backoff_max: f64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        let min_delay_in_s = 1;
        let max_delay_in_s = 300;

        Self {
            min_delay: Duration::from_secs(min_delay_in_s),
            max_delay: Duration::from_secs(max_delay_in_s),         // 5 minutes
            quality_bad_threshold: 0.2,
            quality_offline_threshold: 0.5,
            quality_step: 0.1,
            ignore_timeframe: Duration::from_secs(600),             // 10 minutes
            backoff_exponent: 1.5,
            backoff_min: 2.0,
            backoff_max: max_delay_in_s as f64 / min_delay_in_s as f64,
        }
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PeerOrigin {
    Initialization = 0,
    NetworkRegistry = 1,
    IncomingConnection = 2,
    OutgoingConnection = 3,
    StrategyExistingChannel = 4,
    StrategyConsideringChannel = 5,
    StrategyNewChannel = 6,
    ManualPing = 7,
    Testing = 8,
}

impl std::fmt::Display for PeerOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let description = match self {
            PeerOrigin::Initialization => "node initialization",
            PeerOrigin::NetworkRegistry => "network registry",
            PeerOrigin::IncomingConnection => "incoming connection",
            PeerOrigin::OutgoingConnection => "outgoing connection attempt",
            PeerOrigin::StrategyExistingChannel => "strategy monitors existing channel",
            PeerOrigin::StrategyConsideringChannel => "strategy considers opening a channel",
            PeerOrigin::StrategyNewChannel => "strategy decided to open new channel",
            PeerOrigin::ManualPing => "manual ping",
            PeerOrigin::Testing => "testing",
        };
        write!(f, "{}", description)
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    UNKNOWN = 0,
    /// No connection, default
    RED = 1,
    /// Low quality connection to at least 1 public relay
    ORANGE = 2,
    /// High quality connection to at least 1 public relay
    YELLOW = 3,
    /// High quality connection to at least 1 public relay and 1 NAT node
    GREEN = 4,
}

impl std::fmt::Display for Health {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}


#[cfg_attr(test, mockall::automock)]
pub trait NetworkExternalActions {
    fn is_public(&self, peer: &PeerId) -> bool;

    fn close_connection(&self, peer: &PeerId);

    fn on_peer_offline(&self, peer: &PeerId);

    fn on_network_health_change(&self, old: Health, new: Health);
}

#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
#[derive(Debug, Clone, PartialEq)]
pub struct PeerStatus {
    id: PeerId,
    pub origin: PeerOrigin,
    pub is_public: bool,
    pub last_seen: u64,     // timestamp
    pub quality: f64,
    pub heartbeats_sent: u64,
    pub heartbeats_succeeded: u64,
    pub backoff: f64,
}

impl PeerStatus {
    fn new(id: PeerId, origin: PeerOrigin, backoff: f64) -> PeerStatus {
        PeerStatus {
            id,
            origin,
            is_public: false,
            heartbeats_sent: 0,
            heartbeats_succeeded: 0,
            last_seen: 0,
            backoff,
            quality: 0.0,
        }
    }
}

impl std::fmt::Display for PeerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Entry: [id={}, origin={}, last seen on={}, quality={}, heartbeats sent={}, heartbeats succeeded={}, backoff={}]",
               self.id, self.origin, self.last_seen, self.quality, self.heartbeats_sent, self.heartbeats_succeeded, self.backoff)
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
pub struct Network {
    me: PeerId,
    cfg: NetworkConfig,
    entries: HashMap<String, PeerStatus>,
    ignored: HashMap<String, u64>,      // timestamp
    excluded: HashSet<String>,
    good_quality_public: HashSet<PeerId>,
    bad_quality_public: HashSet<PeerId>,
    good_quality_non_public: HashSet<PeerId>,
    bad_quality_non_public: HashSet<PeerId>,
    last_health: Health,
    network_actions_api: Box<dyn NetworkExternalActions>,
    metric_network_health: Option<SimpleGauge>,
    metric_peers_by_quality: Option<MultiGauge>,
    metric_peer_count: Option<SimpleGauge>
}

impl Network {
    pub fn new(
        my_peer_id: PeerId,
        network_quality_threshold: f64,
        network_actions_api: Box<dyn NetworkExternalActions>
    ) -> Network {
        let cfg = NetworkConfig {
            quality_offline_threshold: network_quality_threshold,
            ..NetworkConfig::default()
        };

        if cfg.quality_offline_threshold < cfg.quality_bad_threshold {
            panic!("Strict requirement failed, bad quality threshold {} must be lower than quality offline threshold {}",
                   cfg.quality_bad_threshold, cfg.quality_offline_threshold);
        }

        let mut excluded = HashSet::new();
        excluded.insert(my_peer_id.to_string());

        let instance = Network {
            me: my_peer_id,
            cfg,
            entries: HashMap::new(),
            ignored: HashMap::new(),
            excluded,
            good_quality_public: HashSet::new(),
            bad_quality_public: HashSet::new(),
            good_quality_non_public: HashSet::new(),
            bad_quality_non_public: HashSet::new(),
            last_health: Health::UNKNOWN,
            network_actions_api,
            metric_network_health: SimpleGauge::new(
                "core_gauge_network_health", "Connectivity health indicator").ok(),
            metric_peers_by_quality: MultiGauge::new(
                "core_mgauge_peers_by_quality",
                "Number different peer types by quality",
                &["type", "quality"]
            ).ok(),
            metric_peer_count: SimpleGauge::new("core_gauge_num_peers", "Number of all peers").ok()
        };

        instance
    }

    /// Check whether the PeerId is present in the network
    pub fn has(&self, peer: &PeerId) -> bool {
        self.entries.contains_key(peer.to_string().as_str())
    }

    /// Add a new PeerId into the network
    ///
    /// Each PeerId must have an origin specification.
    pub fn add(&mut self, peer: &PeerId, origin: PeerOrigin) {
        let id = peer.to_string();
        let now = current_timestamp();

        // assumes disjoint sets
        let has_entry = self.entries.contains_key(id.as_str());
        let is_excluded = !has_entry && self.excluded.contains(id.as_str());

        if ! is_excluded {
            let is_ignored = if !has_entry && self.ignored.contains_key(id.as_str()) {
                let timestamp = self.ignored.get(id.as_str()).unwrap();
                if Duration::from_millis(now - timestamp) > self.cfg.ignore_timeframe {
                    self.ignored.remove(id.as_str());
                    false
                } else {
                    true
                }
            } else {
                false
            };

            if ! has_entry && ! is_ignored {
                let mut entry = PeerStatus::new(peer.clone(), origin, self.cfg.backoff_min);
                entry.is_public = self.network_actions_api.is_public(&peer);
                self.refresh_network_status(&entry);

                if let Some(x) = self.entries.insert(peer.to_string(), entry) {
                    warn!("Evicting an existing record for {}, this should not happen!", &x);
                }
            }
        }
    }

    /// Remove PeerId from the network
    pub fn remove(&mut self, peer: &PeerId) {
        self.prune_from_network_status(&peer);
        self.entries.remove(peer.to_string().as_str());
    }

    /// Update the PeerId record in the network
    pub fn update(&mut self, peer: &PeerId, ping_result: crate::types::Result) {
        if let Some(existing) = self.entries.get(peer.to_string().as_str()) {
            let mut entry = existing.clone();
            entry.heartbeats_sent = entry.heartbeats_sent + 1;
            entry.is_public = self.network_actions_api.is_public(&peer);

            if ping_result.is_err() {
                entry.backoff = self.cfg.backoff_max.max(entry.backoff.powf(self.cfg.backoff_exponent));
                entry.quality = 0.0_f64.max(entry.quality - self.cfg.quality_step);

                if entry.quality < (self.cfg.quality_step / 2.0) {
                    self.network_actions_api.close_connection(&entry.id);
                    self.prune_from_network_status(&entry.id);
                    self.entries.remove(entry.id.to_string().as_str());
                    return
                } else if entry.quality < self.cfg.quality_bad_threshold {
                    self.ignored.insert(entry.id.to_string(), current_timestamp());
                } else if entry.quality < self.cfg.quality_offline_threshold {
                    self.network_actions_api.on_peer_offline(&entry.id);
                }
            } else {
                entry.last_seen = ping_result.ok().unwrap();
                entry.heartbeats_succeeded = entry.heartbeats_succeeded + 1;
                entry.backoff = self.cfg.backoff_min;
                entry.quality = 1.0_f64.min(entry.quality + self.cfg.quality_step);
            }

            self.refresh_network_status(&entry);
            self.entries.insert(entry.id.to_string(), entry);
        } else {
            info!("Ignoring update request for unknown peer {:?}", peer);
        }
    }

    /// Update the internally perceived network status that is processed to the network health
    fn refresh_network_status(&mut self, entry: &PeerStatus) {
        self.prune_from_network_status(&entry.id);

        if entry.quality < self.cfg.quality_offline_threshold {
            if entry.is_public {
                self.bad_quality_public.insert(entry.id.clone());
            } else {
                self.bad_quality_non_public.insert(entry.id.clone());
            }
        } else {
            if entry.is_public {
                self.good_quality_public.insert(entry.id.clone());
            } else {
                self.good_quality_non_public.insert(entry.id.clone());
            }
        }

        let good_public = self.good_quality_public.len();
        let good_non_public = self.good_quality_non_public.len();
        let bad_public = self.bad_quality_public.len();
        let bad_non_public = self.bad_quality_non_public.len();
        let mut health = Health::RED;

        if bad_public > 0 {
            health = Health::ORANGE;
        }

        if good_public > 0 {
            health = if good_non_public > 0 || self.network_actions_api.is_public(&self.me) {
                Health::GREEN
            } else {
                Health::YELLOW
            };
        }

        if health != self.last_health {
            info!("Network health changed from {} to {}", self.last_health, health);
            self.network_actions_api.on_network_health_change(self.last_health, health);
            self.last_health = health;
        }

        // metrics
        if let Some(metric_peer_count) = &self.metric_peer_count {
            metric_peer_count.set((good_public + good_non_public + bad_public + bad_non_public) as f64);
        }

        if let Some(metric_peers_by_quality) = &self.metric_peers_by_quality {
            metric_peers_by_quality.set(&["public", "high"], good_public as f64);
            metric_peers_by_quality.set(&["public", "low"], bad_public as f64);
            metric_peers_by_quality.set(&["nonPublic", "high"], good_non_public as f64);
            metric_peers_by_quality.set(&["nonPublic", "low"], bad_non_public as f64);
        }

        if let Some(metric_network_health) = &self.metric_network_health {
            metric_network_health.set((health as i32).into());
        }
    }

    /// Remove the PeerId from network status observed variables
    fn prune_from_network_status(&mut self, peer: &PeerId) {
        self.good_quality_public.remove(&peer);
        self.good_quality_non_public.remove(&peer);
        self.good_quality_public.remove(&peer);
        self.bad_quality_non_public.remove(&peer);
    }

    pub fn get_peer_status(&self, peer: &PeerId) -> Option<PeerStatus> {
        return match self.entries.get(peer.to_string().as_str()) {
            Some(entry) => Some(entry.clone()),
            None => None
        }
    }

    /// Perform arbitrary predicate filtering operation on the network entries
    pub fn filter<F>(&self, f: F) -> Vec<PeerId>
    where
        F: FnMut(&&PeerStatus) -> bool
    {
        self.entries.values()
            .filter(f)
            .map(|x| { x.id.clone() })
            .collect::<Vec<_>>()
    }

    pub fn find_peers_to_ping(&self, threshold: u64) -> Vec<PeerId> {
        let mut data: Vec<PeerId> = self.filter(|v| {
            let backoff = v.backoff.powf(self.cfg.backoff_exponent);
            let delay = std::cmp::min(
                self.cfg.min_delay * (backoff as u32),
                self.cfg.max_delay,
            );

            (v.last_seen + (delay.as_millis() as u64)) < threshold
        });
        data.sort_by(|a, b| {
            if self.entries.get(a.to_string().as_str()).unwrap().last_seen < self.entries.get(b.to_string().as_str()).unwrap().last_seen {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });

        data
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
impl Network {
    /// Total count of the peers observed withing the network
    #[wasm_bindgen]
    pub fn length(&self) -> usize {
        self.entries.len()
    }

    /// Returns the quality of the network as a network health indicator.
    #[wasm_bindgen]
    pub fn health(&self) -> Health {
        self.last_health
    }

    #[wasm_bindgen]
    pub fn debug_output(&self) -> String {
        let mut output = "".to_string();

        for (_, entry) in &self.entries {
            output.push_str(format!("{}\n", entry).as_str());
        }

        output
    }
}

#[cfg(feature = "wasm")]
pub mod wasm {
    use super::*;
    use std::str::FromStr;
    use js_sys::{JsString};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    impl PeerStatus {
        #[wasm_bindgen]
        pub fn peer_id(&self) -> String {
            self.id.to_base58()
        }
    }

    #[wasm_bindgen]
    impl PeerStatus {
        #[wasm_bindgen]
        pub fn build(peer: JsString,
                     origin: PeerOrigin, is_public: bool, last_seen: u64,
                     quality: f64, heartbeats_sent: u64, heartbeats_succeeded: u64,
                     backoff: f64) -> Self {
            let peer = PeerId::from_str(peer.as_string().unwrap().as_ref()).ok().unwrap();
            Self {
                id: peer,
                origin,
                is_public,
                last_seen,
                quality,
                heartbeats_sent,
                heartbeats_succeeded,
                backoff,
            }
        }
    }

    #[wasm_bindgen]
    struct WasmNetworkApi {
        on_peer_offline_cb: js_sys::Function,
        on_network_health_change_cb: js_sys::Function,
        is_public_cb: js_sys::Function,
        close_connection_cb: js_sys::Function,
    }

    impl NetworkExternalActions for WasmNetworkApi {
        fn is_public(&self, peer: &PeerId) -> bool {
            let this = JsValue::null();
            match self.is_public_cb.call1(&this, &JsValue::from(peer.to_base58())) {
                Ok(v) => v.as_bool().unwrap(),
                _ => {
                    warn!("Encountered error when trying to find out whether peer {} is public", peer);
                    false
                }
            }
        }

        fn close_connection(&self, peer: &PeerId) {
            let this = JsValue::null();
            if let Err(err) = self.close_connection_cb.call1(&this, &JsValue::from(peer.to_base58())) {
                error!("Failed to perform close connection for peer {} with: {}", peer, err.as_string().unwrap().as_str())
            };
        }

        fn on_peer_offline(&self, peer: &PeerId) {
            let this = JsValue::null();
            if let Err(err) = self.on_peer_offline_cb.call1(&this, &JsValue::from(peer.to_base58())) {
                error!("Failed to perform on peer offline operation for peer {} with: {}", peer, err.as_string().unwrap().as_str())
            };
        }

        fn on_network_health_change(&self, old: Health, new: Health) {
            let this = JsValue::null();
            let old = JsValue::from(old as i32);
            let new = JsValue::from(new as i32);
            if let Err(err) = self.on_network_health_change_cb.call2(&this, &old, &new) {
                error!("Failed to perform the network health change operation with: {}", err.as_string().unwrap().as_str())
            };
        }
    }

    #[wasm_bindgen]
    impl Network {
        #[wasm_bindgen]
        pub fn build(me: JsString, quality_threshold: f64,
                     on_peer_offline: js_sys::Function,
                     on_network_health_change: js_sys::Function,
                     is_public: js_sys::Function,
                     close_connection: js_sys::Function) -> Self {
            let me: String = me.into();
            let api = Box::new(WasmNetworkApi{
                on_peer_offline_cb: on_peer_offline,
                on_network_health_change_cb: on_network_health_change,
                is_public_cb: is_public,
                close_connection_cb: close_connection,
            });

            Self::new(
                PeerId::from_str(&me).ok().unwrap(),
                quality_threshold,
                api
            )
        }

        #[wasm_bindgen]
        pub fn peers_to_ping(&self, threshold: u64) -> Vec<JsString> {
            self.find_peers_to_ping(threshold)
                .iter()
                .map(|x| { x.to_base58().into() })
                .collect::<Vec<JsString>>()
        }

        #[wasm_bindgen]
        pub fn contains(&self, peer: JsString) -> bool {
            let peer: String = peer.into();
            self.has(&PeerId::from_str(&peer).ok().unwrap())
        }

        #[wasm_bindgen]
        pub fn register(&mut self, peer: JsString, origin: PeerOrigin) {
            let peer: String = peer.into();
            self.add(&PeerId::from_str(&peer).ok().unwrap(), origin)
        }

        #[wasm_bindgen]
        pub fn unregister(&mut self, peer: JsString) {
            let peer: String = peer.into();
            self.remove(&PeerId::from_str(&peer).ok().unwrap())
        }

        #[wasm_bindgen]
        pub fn refresh(&mut self, peer: JsString, timestamp: JsValue) {
            let peer: String = peer.into();
            let result: crate::types::Result = if timestamp.is_undefined() {
                    Err(())
                } else {
                    timestamp.as_f64()
                        .map(|v| v as u64)
                        .ok_or(())
                };
            self.update(&PeerId::from_str(&peer).ok().unwrap(), result)
        }

        #[wasm_bindgen]
        pub fn quality_of(&self, peer: JsString) -> f64 {
            let peer: String = peer.into();
            match self.get_peer_status(&PeerId::from_str(&peer).ok().unwrap()) {
                Some(v) => v.quality,
                _ => 0.0
            }
        }

        #[wasm_bindgen]
        pub fn all(&self)  -> Vec<JsString> {
            self.filter(|_| true)
                .iter()
                .map(|x| { x.to_base58().into() })
                .collect::<Vec<JsString>>()
        }

        #[wasm_bindgen]
        pub fn get_peer_info(&self, peer: JsString) -> Option<PeerStatus> {
            let peer: String = peer.into();
            self.get_peer_status(&PeerId::from_str(&peer).ok().unwrap())
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    struct DummyNetworkAction {}
    
    impl NetworkExternalActions for DummyNetworkAction {
        fn is_public(&self, _: &PeerId) -> bool {
            false
        }

        fn close_connection(&self, _: &PeerId) {
            ()
        }

        fn on_peer_offline(&self, _: &PeerId) {
            ()
        }

        fn on_network_health_change(&self, _: Health, _: Health) {
            ()
        }
    }
    
    fn basic_network(my_id: &PeerId) -> Network {
        Network::new(
            my_id.clone(),
            0.6,
            Box::new(DummyNetworkAction{}),
        )
    }

    #[test]
    fn test_network_health_should_be_ordered_numerically_for_metrics_output() {
        assert_eq!(Health::UNKNOWN as i32, 0);
        assert_eq!(Health::RED as i32, 1);
        assert_eq!(Health::ORANGE as i32, 2);
        assert_eq!(Health::YELLOW as i32, 3);
        assert_eq!(Health::GREEN as i32, 4);
    }

    #[test]
    fn test_network_should_not_contain_the_self_reference() {
        let me = PeerId::random();

        let mut peers = basic_network(&me);

        peers.add(&me, PeerOrigin::IncomingConnection);

        assert_eq!(0, peers.length());
        assert!(! peers.has(&me))
    }

    #[test]
    fn test_network_should_contain_a_registered_peer() {
        let expected = PeerId::random();

        let mut peers = basic_network(&PeerId::random());

        peers.add(&expected, PeerOrigin::IncomingConnection);

        assert_eq!(1, peers.length());
        assert!(peers.has(&expected))
    }

    #[test]
    fn test_network_should_remove_a_peer_on_unregistration() {
        let peer = PeerId::random();

        let mut peers = basic_network(&PeerId::random());

        peers.add(&peer, PeerOrigin::IncomingConnection);

        peers.remove(&peer);

        assert_eq!(0, peers.length());
        assert!(! peers.has(&peer))
    }

    #[test]
    fn test_network_should_ingore_heartbeat_updates_for_peers_that_were_not_registered() {
        let peer = PeerId::random();

        let mut peers = basic_network(&PeerId::random());

        peers.update(&peer, Ok(current_timestamp()));

        assert_eq!(0, peers.length());
        assert!(! peers.has(&peer))
    }

    #[test]
    fn test_network_should_be_able_to_register_a_succeeded_heartbeat_result() {
        let peer = PeerId::random();

        let mut peers = basic_network(&PeerId::random());

        peers.add(&peer, PeerOrigin::IncomingConnection);

        let ts = current_timestamp();

        peers.update(&peer, Ok(ts.clone()));

        let actual = peers.debug_output();

        assert!(actual.contains("heartbeats sent=1"));
        assert!(actual.contains("heartbeats succeeded=1"));
        assert!(actual.contains(format!("last seen on={}", ts).as_str()))
    }

    #[test]
    fn test_network_should_ignore_a_peer_that_has_reached_lower_thresholds_a_specified_amount_of_time() {
        let peer = PeerId::random();

        let mut peers = basic_network(&PeerId::random());

        peers.add(&peer, PeerOrigin::IncomingConnection);

        peers.update(&peer, Ok(current_timestamp()));
        peers.update(&peer, Ok(current_timestamp()));
        peers.update(&peer, Err(()));          // should drop to ignored
        peers.update(&peer, Err(()));          // should drop from network

        assert!(! peers.has(&peer));

        // peer should remain ignored and not be added
        peers.add(&peer, PeerOrigin::IncomingConnection);

        assert!(! peers.has(&peer))
    }

    #[test]
    fn test_network_should_be_able_to_register_a_failed_heartbeat_result() {
        let peer = PeerId::random();
        let mut peers = basic_network(&PeerId::random());

        peers.add(&peer, PeerOrigin::IncomingConnection);

        peers.update(&peer, Ok(current_timestamp()));
        peers.update(&peer, Ok(current_timestamp()));
        peers.update(&peer, Err(()));

        let actual = peers.debug_output();

        assert!(actual.contains("heartbeats succeeded=2"));
        assert!(actual.contains("backoff=300"));
    }

    #[test]
    fn test_network_should_be_listed_for_the_ping_if_last_recorded_later_than_reference() {
        let first = PeerId::random();
        let second = PeerId::random();
        let mut peers = basic_network(&PeerId::random());

        peers.add(&first, PeerOrigin::IncomingConnection);
        peers.add(&second, PeerOrigin::IncomingConnection);

        let ts = current_timestamp();

        let mut expected = vec!(first, second);
        expected.sort();

        peers.update(&first, Ok(ts));
        peers.update(&second, Ok(ts));

        let mut actual = peers.find_peers_to_ping(ts + 3000);
        actual.sort();

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_network_should_have_no_knowledge_about_health_without_any_registered_peers() {
        let peers = basic_network(&PeerId::random());

        assert_eq!(peers.health(), Health::UNKNOWN);
    }

    #[test]
    fn test_network_should_be_unhealthy_without_any_heartbeat_updates() {
        let peer = PeerId::random();

        let mut peers = basic_network(&PeerId::random());

        peers.add(&peer, PeerOrigin::IncomingConnection);

        assert_eq!(peers.health(), Health::RED);
    }

    #[test]
    fn test_network_should_be_unhealthy_without_any_peers_once_the_health_was_known() {
        let peer = PeerId::random();

        let mut peers = basic_network(&PeerId::random());

        peers.add(&peer, PeerOrigin::IncomingConnection);
        let _ = peers.health();
        peers.remove(&peer);

        assert_eq!(peers.health(), Health::RED);
    }

    #[test]
    fn test_network_should_notify_the_callback_for_every_health_change() {
        let peer = PeerId::random();

        let mut mock = MockNetworkExternalActions::new();
        mock.expect_is_public()
            .times(1)
            .returning(|_| { false });
        mock.expect_on_network_health_change()
            .times(1)
            .return_const(());

        let mut peers = Network::new(
            PeerId::random(),
            0.6,
            Box::new(mock),
        );

        peers.add(&peer, PeerOrigin::IncomingConnection);

        assert_eq!(peers.health(), Health::RED);
    }

    #[test]
    fn test_network_should_be_healthy_when_a_public_peer_is_pingable_with_low_quality() {
        let peer = PeerId::random();
        let public = peer.clone();

        let mut mock = MockNetworkExternalActions::new();
        mock.expect_is_public()
            .times(2)
            .returning(move |x| { x == &public });
        mock.expect_on_network_health_change()
            .times(1)
            .return_const(());
        let mut peers = Network::new(
            PeerId::random(),
            0.6,
            Box::new(mock)
        );

        peers.add(&peer, PeerOrigin::IncomingConnection);

        peers.update(&peer, Ok(current_timestamp()));

        assert_eq!(peers.health(), Health::ORANGE);
    }

    #[test]
    fn test_network_should_remove_the_peer_once_it_reaches_the_lowest_possible_quality() {
        let peer = PeerId::random();
        let public = peer.clone();

        let mut mock = MockNetworkExternalActions::new();
        mock.expect_is_public()
            .times(3)
            .returning(move |x| { x == &public });
        mock.expect_on_network_health_change()
            .times(1)
            .return_const(());
        mock.expect_close_connection()
            .times(1)
            .return_const(());

        let mut peers = Network::new(
            PeerId::random(),
            0.6,
            Box::new(mock)
        );

        peers.add(&peer, PeerOrigin::IncomingConnection);

        peers.update(&peer, Ok(current_timestamp()));
        peers.update(&peer, Err(()));

        assert!(! peers.has(&public));
    }

    #[test]
    fn test_network_should_be_healthy_when_a_public_peer_is_pingable_with_high_quality_and_i_am_public() {
        let me = PeerId::random();
        let peer = PeerId::random();
        let public = vec![peer.clone(), me.clone()];

        let mut mock = MockNetworkExternalActions::new();
        mock.expect_is_public()
            .times(5)
            .returning(move |x| { public.contains(&x) });
        mock.expect_on_network_health_change()
            .times(2)
            .return_const(());
        let mut peers = Network::new(
            me,
            0.3,
            Box::new(mock)
        );

        peers.add(&peer, PeerOrigin::IncomingConnection);

        for _ in 0..3 {
            peers.update(&peer, Ok(current_timestamp()));
        }

        assert_eq!(peers.health(), Health::GREEN);
    }

    #[test]
    fn test_network_should_be_healthy_when_a_public_peer_is_pingable_with_high_quality_and_another_high_quality_non_public() {
        let peer = PeerId::random();
        let peer2 = PeerId::random();
        let public = vec![peer.clone()];

        let mut mock = MockNetworkExternalActions::new();

        mock.expect_is_public()
            .times(8)
            .returning(move |x| { public.contains(&x) });
        mock.expect_on_network_health_change()
            .times(2)
            .return_const(());
        let mut peers = Network::new(
            PeerId::random(),
            0.3,
            Box::new(mock)
        );

        peers.add(&peer, PeerOrigin::IncomingConnection);
        peers.add(&peer2, PeerOrigin::IncomingConnection);

        for _ in 0..3 {
            peers.update(&peer2, Ok(current_timestamp()));
            peers.update(&peer, Ok(current_timestamp()));
        }

        assert_eq!(peers.health(), Health::GREEN);
    }
}