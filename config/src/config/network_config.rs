// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::{
    config::{PersistableConfig, RoleType, RootPath, SecureBackend},
    keys::KeyPair,
    utils,
};
use anyhow::{anyhow, ensure, Result};
use libra_crypto::{x25519, Uniform};
use libra_network_address::NetworkAddress;
use libra_types::{transaction::authenticator::AuthenticationKey, PeerId};
use rand::{
    rngs::{OsRng, StdRng},
    Rng, SeedableRng,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, convert::TryFrom, path::PathBuf, string::ToString};

const NETWORK_PEERS_DEFAULT: &str = "network_peers.config.toml";
const SEED_PEERS_DEFAULT: &str = "seed_peers.toml";

/// Current supported protocol negotiation handshake version.
///
/// See [`perform_handshake`] in `network/src/transport.rs`
// TODO(philiphayes): ideally this constant lives somewhere in network/ ...
// might need to extract into a separate network_constants crate or something.
pub const HANDSHAKE_VERSION: u8 = 0;

#[cfg_attr(any(test, feature = "fuzzing"), derive(Clone, PartialEq))]
#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    // TODO: Add support for multiple listen/advertised addresses in config.
    // The address that this node is listening on for new connections.
    pub listen_address: NetworkAddress,
    // The address that this node advertises to other nodes for the discovery protocol.
    pub advertised_address: NetworkAddress,
    pub discovery_interval_ms: u64,
    pub connectivity_check_interval_ms: u64,
    // If the network uses remote authentication, only trusted peers are allowed to connect.
    // Otherwise, any node can connect.
    pub enable_remote_authentication: bool,
    // Enable this network to use either gossip discovery or onchain discovery.
    pub discovery_method: DiscoveryMethod,
    // network peers are the nodes allowed to connect when the network is started in authenticated
    // mode.
    #[serde(skip)]
    pub network_peers: NetworkPeersConfig,
    pub network_peers_file: PathBuf,
    // seed_peers act as seed nodes for the discovery protocol.
    #[serde(skip)]
    pub seed_peers: SeedPeersConfig,
    pub seed_peers_file: PathBuf,
    pub identity: Identity,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        let mut config = Self {
            listen_address: "/ip4/0.0.0.0/tcp/6180".parse().unwrap(),
            advertised_address: "/ip4/127.0.0.1/tcp/6180".parse().unwrap(),
            discovery_interval_ms: 1000,
            connectivity_check_interval_ms: 5000,
            enable_remote_authentication: true,
            discovery_method: DiscoveryMethod::Gossip,
            identity: Identity::None,
            network_peers_file: PathBuf::new(),
            network_peers: NetworkPeersConfig::default(),
            seed_peers_file: PathBuf::new(),
            seed_peers: SeedPeersConfig::default(),
        };
        config.prepare_identity();
        config
    }
}

impl NetworkConfig {
    /// This clones the underlying data except for the key so that this config can be used as a
    /// template for another config.
    pub fn clone_for_template(&self) -> Self {
        Self {
            listen_address: self.listen_address.clone(),
            advertised_address: self.advertised_address.clone(),
            discovery_interval_ms: self.discovery_interval_ms,
            connectivity_check_interval_ms: self.connectivity_check_interval_ms,
            enable_remote_authentication: self.enable_remote_authentication,
            discovery_method: self.discovery_method,
            identity: Identity::None,
            network_peers_file: self.network_peers_file.clone(),
            network_peers: self.network_peers.clone(),
            seed_peers_file: self.seed_peers_file.clone(),
            seed_peers: self.seed_peers.clone(),
        }
    }

    pub fn load(&mut self, root_dir: &RootPath, network_role: RoleType) -> Result<()> {
        if !self.network_peers_file.as_os_str().is_empty() {
            let path = root_dir.full_path(&self.network_peers_file);
            self.network_peers = NetworkPeersConfig::load_config(&path)?;
        }
        if !self.seed_peers_file.as_os_str().is_empty() {
            let path = root_dir.full_path(&self.seed_peers_file);
            self.seed_peers = SeedPeersConfig::load_config(&path)?;
            self.seed_peers.verify_libranet_addrs()?;
        }
        if self.advertised_address.to_string().is_empty() {
            self.advertised_address =
                utils::get_local_ip().ok_or_else(|| anyhow!("No local IP"))?;
        }
        if self.listen_address.to_string().is_empty() {
            self.listen_address = utils::get_local_ip().ok_or_else(|| anyhow!("No local IP"))?;
        }

        if network_role.is_validator() {
            ensure!(
                self.network_peers_file.as_os_str().is_empty(),
                "Validators should not define network_peers_file"
            );
            ensure!(
                self.network_peers.peers.is_empty(),
                "Validators should not define network_peers"
            );
        }

        self.prepare_identity();
        Ok(())
    }

    fn prepare_identity(&mut self) {
        match &mut self.identity {
            Identity::FromStorage(_) => (),
            Identity::None => {
                let mut rng = StdRng::from_seed(OsRng.gen());
                let key = x25519::PrivateKey::generate(&mut rng);
                let peer_id = AuthenticationKey::try_from(key.public_key().as_slice())
                    .unwrap()
                    .derived_address();
                self.identity = Identity::from_config(key, peer_id);
            }
            Identity::FromConfig(config) => {
                let pubkey = config.keypair.public_key();
                let peer_id = AuthenticationKey::try_from(pubkey.as_slice())
                    .unwrap()
                    .derived_address();

                if config.peer_id == PeerId::default() {
                    config.peer_id = peer_id;
                }
            }
        };
    }

    fn default_path(&self, config_path: &str) -> String {
        let peer_id = self.identity.peer_id_from_config().unwrap_or_default();
        format!("{}.{}", peer_id.to_string(), config_path)
    }

    pub fn save(&mut self, root_dir: &RootPath) -> Result<()> {
        if self.network_peers != NetworkPeersConfig::default() {
            if self.network_peers_file.as_os_str().is_empty() {
                let file_name = self.default_path(NETWORK_PEERS_DEFAULT);
                self.network_peers_file = PathBuf::from(file_name);
            }
            let path = root_dir.full_path(&self.network_peers_file);
            self.network_peers.save_config(&path)?;
        }

        if self.seed_peers_file.as_os_str().is_empty() {
            let file_name = self.default_path(SEED_PEERS_DEFAULT);
            self.seed_peers_file = PathBuf::from(file_name);
        }
        let path = root_dir.full_path(&self.seed_peers_file);
        self.seed_peers.save_config(&path)?;
        Ok(())
    }

    pub fn random(&mut self, rng: &mut StdRng) {
        self.random_with_peer_id(rng, None);
    }

    pub fn random_with_peer_id(&mut self, rng: &mut StdRng, peer_id: Option<PeerId>) {
        let identity_key = x25519::PrivateKey::generate(rng);
        let peer_id = if let Some(peer_id) = peer_id {
            peer_id
        } else {
            AuthenticationKey::try_from(identity_key.public_key().as_slice())
                .unwrap()
                .derived_address()
        };
        self.identity = Identity::from_config(identity_key, peer_id);
    }

    #[cfg(any(test, feature = "fuzzing"))]
    pub fn peer_id(&self) -> PeerId {
        self.identity.peer_id_from_config().unwrap()
    }
}

// This is separated to another config so that it can be written to its own file
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SeedPeersConfig {
    // All peers config. Key:a unique peer id, will be PK in future, Value: peer discovery info
    pub seed_peers: HashMap<PeerId, Vec<NetworkAddress>>,
}

impl SeedPeersConfig {
    /// Check that all seed peer addresses look like canonical LibraNet addresses
    pub fn verify_libranet_addrs(&self) -> Result<()> {
        for (peer_id, addrs) in self.seed_peers.iter() {
            for addr in addrs {
                ensure!(
                    addr.is_libranet_addr(),
                    "Unexpected seed peer address format: peer_id: {}, addr: '{}'",
                    peer_id.short_str(),
                    addr,
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
pub struct NetworkPeersConfig {
    #[serde(flatten)]
    #[serde(serialize_with = "utils::serialize_ordered_map")]
    pub peers: HashMap<PeerId, NetworkPeerInfo>,
}

impl std::fmt::Debug for NetworkPeersConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "<{} keys>", self.peers.len())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NetworkPeerInfo {
    #[serde(rename = "ni")]
    pub identity_public_key: x25519::PublicKey,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMethod {
    // default until we can deprecate
    Gossip,
    Onchain,
    None,
}

#[cfg_attr(any(test, feature = "fuzzing"), derive(Clone, PartialEq))]
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum Identity {
    FromConfig(IdentityFromConfig),
    FromStorage(IdentityFromStorage),
    None,
}

impl Identity {
    pub fn from_config(key: x25519::PrivateKey, peer_id: PeerId) -> Self {
        let keypair = KeyPair::load(key);
        Identity::FromConfig(IdentityFromConfig { keypair, peer_id })
    }

    pub fn from_storage(key_name: String, peer_id_name: String, backend: SecureBackend) -> Self {
        Identity::FromStorage(IdentityFromStorage {
            key_name,
            peer_id_name,
            backend,
        })
    }

    pub fn peer_id_from_config(&self) -> Option<PeerId> {
        match self {
            Identity::FromConfig(config) => Some(config.peer_id),
            _ => None,
        }
    }

    pub fn public_key_from_config(&self) -> Option<x25519::PublicKey> {
        if let Identity::FromConfig(config) = self {
            Some(config.keypair.public_key())
        } else {
            None
        }
    }
}

/// The identity is stored within the config.
#[cfg_attr(any(test, feature = "fuzzing"), derive(Clone, PartialEq))]
#[derive(Debug, Deserialize, Serialize)]
pub struct IdentityFromConfig {
    #[serde(rename = "key")]
    pub keypair: KeyPair<x25519::PrivateKey>,
    pub peer_id: PeerId,
}

/// This represents an identity in a secure-storage as defined in NodeConfig::secure.
#[cfg_attr(any(test, feature = "fuzzing"), derive(Clone, PartialEq))]
#[derive(Debug, Deserialize, Serialize)]
pub struct IdentityFromStorage {
    pub key_name: String,
    pub peer_id_name: String,
    pub backend: SecureBackend,
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::config::RoleType;
    use libra_temppath::TempPath;
    use rand::{rngs::StdRng, SeedableRng};

    #[test]
    fn test_with_defaults() {
        // Assert default exists
        let (mut config, path) = generate_config();
        assert_eq!(config.network_peers, NetworkPeersConfig::default());
        assert_eq!(config.network_peers_file, PathBuf::new());
        assert_ne!(config.identity, Identity::None);
        assert_eq!(config.seed_peers, SeedPeersConfig::default());
        assert_eq!(config.seed_peers_file, PathBuf::new());
        let identity = config.identity.clone();

        // Assert default loading doesn't affect paths and defaults remain in place
        let root_dir = RootPath::new_path(path.path());
        config.load(&root_dir, RoleType::FullNode).unwrap();
        assert_eq!(config.network_peers, NetworkPeersConfig::default());
        assert_eq!(config.network_peers_file, PathBuf::new());
        assert_eq!(config.identity, identity);
        assert_eq!(config.seed_peers_file, PathBuf::new());
        assert_eq!(config.seed_peers, SeedPeersConfig::default());

        // Assert saving updates paths
        config.save(&root_dir).unwrap();
        assert_eq!(config.seed_peers, SeedPeersConfig::default());
        assert_eq!(
            config.seed_peers_file,
            PathBuf::from(config.default_path(SEED_PEERS_DEFAULT))
        );

        // Assert paths and values are not set (i.e., no defaults apply)
        assert_eq!(config.identity, identity);
        assert_eq!(config.network_peers, NetworkPeersConfig::default());
        assert_eq!(config.network_peers_file, PathBuf::new());
    }

    #[test]
    fn test_with_random() {
        let (mut config, path) = generate_config();
        config.network_peers = NetworkPeersConfig::default();
        let mut rng = StdRng::from_seed([5u8; 32]);
        config.random(&mut rng);
        // This is default (empty) otherwise
        let peer_id = config.identity.peer_id_from_config().unwrap();
        config.seed_peers.seed_peers.insert(peer_id, vec![]);

        let identity = config.identity.clone();
        let peers = config.network_peers.clone();
        let seed_peers = config.seed_peers.clone();

        // Assert empty paths
        assert_eq!(config.network_peers_file, PathBuf::new());
        assert_eq!(config.seed_peers_file, PathBuf::new());

        // Assert saving updates paths
        let root_dir = RootPath::new_path(path.path());
        config.save(&root_dir).unwrap();
        assert_eq!(config.identity, identity);
        assert_eq!(config.network_peers, peers);
        assert_eq!(config.network_peers_file, PathBuf::new());
        assert_eq!(config.seed_peers, seed_peers);
        assert_eq!(
            config.seed_peers_file,
            PathBuf::from(config.default_path(SEED_PEERS_DEFAULT))
        );

        // Assert a fresh load correctly populates the config
        let mut new_config = NetworkConfig::default();
        // First that paths are empty
        assert_eq!(new_config.network_peers_file, PathBuf::new());
        assert_eq!(new_config.seed_peers_file, PathBuf::new());
        // Loading populates things correctly
        let result = new_config.load(&root_dir, RoleType::Validator);
        result.unwrap();
        assert_eq!(config.identity, identity);
        assert_eq!(config.network_peers, peers);
        assert_eq!(config.network_peers_file, PathBuf::new(),);
        assert_eq!(config.seed_peers, seed_peers);
        assert_eq!(
            config.seed_peers_file,
            PathBuf::from(config.default_path(SEED_PEERS_DEFAULT))
        );
    }

    #[test]
    fn test_generate_ip_addresses_on_load() {
        // Generate a random node
        let (mut config, path) = generate_config();
        let mut rng = StdRng::from_seed([32u8; 32]);
        config.random(&mut rng);
        let root_dir = RootPath::new_path(path.path());

        // Now reset IP addresses and save
        config.listen_address = NetworkAddress::mock();
        config.advertised_address = NetworkAddress::mock();
        config.save(&root_dir).unwrap();

        // Now load and verify default IP addresses are generated
        config.load(&root_dir, RoleType::FullNode).unwrap();
        assert_ne!(config.listen_address.to_string(), "");
        assert_ne!(config.advertised_address.to_string(), "");
    }

    fn generate_config() -> (NetworkConfig, TempPath) {
        let temp_dir = TempPath::new();
        temp_dir.create_as_dir().expect("error creating tempdir");
        let mut config = NetworkConfig::default();
        config.network_peers = NetworkPeersConfig::default();
        (config, temp_dir)
    }
}
