//! The local certificate authority used to intercept HTTPS.
//!
//! One ECDSA P-256 CA per data dir (10 years), one shared leaf key, and a bounded
//! per-host cache of ready-to-use TLS server configs. Minting a leaf is an ECDSA
//! signature (~0.1 ms), so even cache misses are cheap.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::{fs, io};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PublicKeyData, SerialNumber,
};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::ServerConfig;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use time::{Duration, OffsetDateTime};

const CACHE_MAX: usize = 1024;
const CA_YEARS: i64 = 10;
/// Browsers reject leaf certificates valid for more than 398 days.
const LEAF_DAYS: i64 = 365;

pub(crate) struct Ca {
    issuer: Issuer<'static, KeyPair>,
    ca_der: CertificateDer<'static>,
    pem: String,
    fingerprint: String,
    leaf_key: KeyPair,
    /// base64 SHA-256 of the CA's and the leaf's public keys, comma-separated: what
    /// Chromium's `--ignore-certificate-errors-spki-list` wants.
    spki_hashes: String,
    provider: Arc<CryptoProvider>,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    configs: HashMap<String, Arc<ServerConfig>>,
    order: VecDeque<String>,
}

impl Ca {
    /// Loads `dir/ca.crt` + `dir/ca.key`, or creates them on first run.
    pub fn load_or_create(dir: &Path, provider: Arc<CryptoProvider>) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let (crt_path, key_path) = (dir.join("ca.crt"), dir.join("ca.key"));
        let (pem, key) = if crt_path.exists() {
            let key =
                KeyPair::from_pem(&fs::read_to_string(&key_path)?).map_err(io::Error::other)?;
            (fs::read_to_string(&crt_path)?, key)
        } else {
            create(&crt_path, &key_path)?
        };
        let ca_der = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(io::Error::other)?;
        let leaf_key = KeyPair::generate().map_err(io::Error::other)?;
        let spki_hashes = [
            key.subject_public_key_info(),
            leaf_key.subject_public_key_info(),
        ]
        .iter()
        .map(|der| {
            let digest = ring::digest::digest(&ring::digest::SHA256, der);
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, digest)
        })
        .collect::<Vec<_>>()
        .join(",");
        let issuer = Issuer::from_ca_cert_pem(&pem, key).map_err(io::Error::other)?;
        Ok(Self {
            fingerprint: fingerprint(&ca_der),
            issuer,
            ca_der,
            pem,
            leaf_key,
            spki_hashes,
            provider,
            cache: Mutex::default(),
        })
    }

    pub fn cert_pem(&self) -> &str {
        &self.pem
    }

    pub fn spki_hashes(&self) -> &str {
        &self.spki_hashes
    }

    /// SHA-256 of the CA certificate, `AB:CD:...`, to compare with what is installed.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// A TLS config presenting a certificate for `host` (DNS name or IP address).
    pub fn server_config(&self, host: &str) -> io::Result<Arc<ServerConfig>> {
        let host = host.to_ascii_lowercase();
        if let Some(config) = self.cache.lock().expect("ca cache").configs.get(&host) {
            return Ok(config.clone());
        }
        let config = Arc::new(self.mint(&host)?);
        let mut cache = self.cache.lock().expect("ca cache");
        if cache.order.len() >= CACHE_MAX
            && let Some(oldest) = cache.order.pop_front()
        {
            cache.configs.remove(&oldest);
        }
        if cache.configs.insert(host.clone(), config.clone()).is_none() {
            cache.order.push_back(host);
        }
        Ok(config)
    }

    fn mint(&self, host: &str) -> io::Result<ServerConfig> {
        let mut params =
            CertificateParams::new(vec![host.to_string()]).map_err(io::Error::other)?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.serial_number = Some(random_serial()?);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        params.not_after = now + Duration::days(LEAF_DAYS);
        let leaf = params
            .signed_by(&self.leaf_key, &self.issuer)
            .map_err(io::Error::other)?;

        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.leaf_key.serialize_der()));
        let mut config = ServerConfig::builder_with_provider(self.provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone(), self.ca_der.clone()], key)
            .map_err(io::Error::other)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(config)
    }
}

fn create(crt_path: &Path, key_path: &Path) -> io::Result<(String, KeyPair)> {
    let key = KeyPair::generate().map_err(io::Error::other)?;
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "shiftWire CA");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "shiftWire local interception CA");
    params.serial_number = Some(random_serial()?);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(365 * CA_YEARS);
    let cert = params.self_signed(&key).map_err(io::Error::other)?;

    // Key first: a crash in between leaves no certificate without its key.
    write_private(key_path, key.serialize_pem().as_bytes())?;
    write_atomic(crt_path, cert.pem().as_bytes())?;
    Ok((cert.pem(), key))
}

/// Readers never see a half-written file: write aside, then rename.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)
}

fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(tmp, path)
}

/// Unique serials matter: Firefox rejects two different certs with the same issuer + serial.
fn random_serial() -> io::Result<SerialNumber> {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| io::Error::other("no OS randomness"))?;
    bytes[0] &= 0x7f; // keep it positive
    Ok(SerialNumber::from_slice(&bytes))
}

fn fingerprint(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    let hex: Vec<String> = digest.as_ref().iter().map(|b| format!("{b:02X}")).collect();
    hex.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> Arc<CryptoProvider> {
        Arc::new(rustls::crypto::ring::default_provider())
    }

    #[test]
    fn created_once_then_reloaded() {
        let dir = std::env::temp_dir().join(format!("shiftwire-ca-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let first = Ca::load_or_create(&dir, provider()).unwrap();
        let again = Ca::load_or_create(&dir, provider()).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert!(first.cert_pem().starts_with("-----BEGIN CERTIFICATE-----"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mints_for_names_and_ips_and_caches() {
        let dir = std::env::temp_dir().join(format!("shiftwire-ca-mint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ca = Ca::load_or_create(&dir, provider()).unwrap();
        let a = ca.server_config("Example.com").unwrap();
        let b = ca.server_config("example.com").unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "second lookup is served from the cache"
        );
        ca.server_config("10.0.0.7").unwrap();
        ca.server_config("::1").unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
