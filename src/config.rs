use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const OCI_CONFIG_SECTION: &str = "DEFAULT";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppConfig {
    #[serde(default)]
    pub app: AppSection,
    pub oci: OciProfileConfig,
    #[serde(default)]
    pub instance: InstanceConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
}

impl AppConfig {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file: {}", path.display()))
    }

    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        let contents = toml::to_string_pretty(self).context("failed to serialize app config")?;
        fs::write(path, contents)
            .with_context(|| format!("failed to write config file: {}", path.display()))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct AppSection {
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OciProfileConfig {
    #[serde(default)]
    pub config_file: Option<PathBuf>,
    #[serde(default = "default_oci_profile")]
    pub profile: String,
}

impl OciProfileConfig {
    pub fn resolve_credentials(&self) -> Result<OciCredentials> {
        let path = self
            .config_file
            .clone()
            .map(Ok)
            .unwrap_or_else(find_default_oci_config_path)?;
        let config = OciIniConfig::load_from_path(&path)?;
        config
            .profiles
            .get(&self.profile)
            .cloned()
            .with_context(|| {
                format!(
                    "OCI profile '{}' not found in {}",
                    self.profile,
                    path.display()
                )
            })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OciCredentials {
    pub user: String,
    pub fingerprint: String,
    pub tenancy: String,
    pub region: String,
    pub key_file: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OciIniConfig {
    pub profiles: HashMap<String, OciCredentials>,
}

impl OciIniConfig {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read OCI config file: {}", path.display()))?;

        let mut profiles = HashMap::new();
        let mut current_section: Option<String> = None;
        let mut current_fields: HashMap<String, String> = HashMap::new();

        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                if let Some(section) = current_section.take() {
                    let profile = parse_profile(&section, &current_fields)?;
                    profiles.insert(section, profile);
                    current_fields.clear();
                }
                current_section = Some(trimmed[1..trimmed.len() - 1].trim().to_string());
                continue;
            }

            let Some((key, value)) = trimmed.split_once('=') else {
                bail!("invalid OCI config line: {trimmed}");
            };
            current_fields.insert(key.trim().to_string(), value.trim().to_string());
        }

        if let Some(section) = current_section {
            let profile = parse_profile(&section, &current_fields)?;
            profiles.insert(section, profile);
        }

        if profiles.is_empty() {
            bail!("OCI config file has no profiles: {}", path.display());
        }

        Ok(Self { profiles })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct TelegramConfig {
    #[serde(default)]
    pub bot_token: Option<String>,
    #[serde(default)]
    pub mode: TelegramMode,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub language_preferences: HashMap<i64, String>,
}

impl TelegramConfig {
    pub fn preferred_locale(&self, chat_id: i64, fallback: &str) -> String {
        self.language_preferences
            .get(&chat_id)
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    }

    pub fn set_preferred_locale(&mut self, chat_id: i64, locale: impl Into<String>) {
        self.language_preferences.insert(chat_id, locale.into());
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TelegramMode {
    #[default]
    Polling,
    Webhook,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct InstanceConfig {
    #[serde(default)]
    pub launch: Option<LaunchInstanceConfig>,
    #[serde(default)]
    pub free_tier_defaults: FreeTierDefaults,
}

impl InstanceConfig {
    pub fn effective_launch_config(&self) -> LaunchMode {
        match &self.launch {
            Some(config) => LaunchMode::Explicit(config.clone()),
            None => LaunchMode::FreeTierFallback(self.free_tier_defaults.clone()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum LaunchMode {
    Explicit(LaunchInstanceConfig),
    FreeTierFallback(FreeTierDefaults),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FreeTierDefaults {
    #[serde(default = "default_shape_candidates")]
    pub shape_candidates: Vec<DefaultShapeCandidate>,
    #[serde(default)]
    pub assign_public_ip: bool,
}

impl Default for FreeTierDefaults {
    fn default() -> Self {
        Self {
            shape_candidates: default_shape_candidates(),
            assign_public_ip: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DefaultShapeCandidate {
    pub shape: String,
    pub ocpus: u32,
    pub memory_in_gbs: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct LaunchInstanceConfig {
    pub availability_domain: String,
    pub compartment_id: String,
    pub subnet_id: String,
    pub image_id: String,
    pub display_name: String,
    pub ssh_authorized_keys: String,
    #[serde(default)]
    pub shape: Option<String>,
    #[serde(default)]
    pub shape_config: Option<ShapeConfig>,
    #[serde(default)]
    pub boot_volume_size_in_gbs: Option<u32>,
    #[serde(default)]
    pub boot_volume_vpus_per_gb: Option<u32>,
    #[serde(default)]
    pub assign_public_ip: bool,
    #[serde(default)]
    pub assign_private_dns_record: bool,
    #[serde(default)]
    pub assign_ipv6_ip: bool,
    #[serde(default)]
    pub ipv6_subnet_cidr: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ShapeConfig {
    pub ocpus: u32,
    pub memory_in_gbs: u32,
}

fn parse_profile(section: &str, fields: &HashMap<String, String>) -> Result<OciCredentials> {
    let required = |key: &str| {
        fields
            .get(key)
            .cloned()
            .with_context(|| format!("missing '{key}' in OCI profile [{section}]"))
    };

    Ok(OciCredentials {
        user: required("user")?,
        fingerprint: required("fingerprint")?,
        tenancy: required("tenancy")?,
        region: required("region")?,
        key_file: PathBuf::from(required("key_file")?),
    })
}

fn find_default_oci_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    let candidates = [home.join(".oci/config"), home.join(".config/oci/config")];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .with_context(|| {
            format!(
                "no OCI config file found; tried {} and {}",
                home.join(".oci/config").display(),
                home.join(".config/oci/config").display()
            )
        })
}

fn default_locale() -> String {
    "en".to_string()
}

fn default_log_dir() -> PathBuf {
    PathBuf::from("logs")
}

fn default_oci_profile() -> String {
    OCI_CONFIG_SECTION.to_string()
}

fn default_shape_candidates() -> Vec<DefaultShapeCandidate> {
    vec![
        DefaultShapeCandidate {
            shape: "VM.Standard.A1.Flex".to_string(),
            ocpus: 4,
            memory_in_gbs: 24,
        },
        DefaultShapeCandidate {
            shape: "VM.Standard.E2.1.Micro".to_string(),
            ocpus: 1,
            memory_in_gbs: 1,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::tempdir;

    #[test]
    fn loads_app_config_with_explicit_launch_config() {
        let config = AppConfig::load_from_path(
            write_temp_file(
                r#"
[app]
locale = "zh-CN"

[oci]
config_file = "/tmp/oci-config"

[instance.launch]
availability_domain = "AD-1"
compartment_id = "ocid1.compartment.oc1..example"
subnet_id = "ocid1.subnet.oc1..example"
image_id = "ocid1.image.oc1..example"
display_name = "oci-cct-app-01"
ssh_authorized_keys = "ssh-rsa AAA..."
shape = "VM.Standard.A1.Flex"
assign_public_ip = true

[instance.launch.shape_config]
ocpus = 4
memory_in_gbs = 24
"#,
            )
            .as_path(),
        )
        .unwrap();

        assert_eq!(config.app.locale, "zh-CN");
        assert_eq!(
            config.instance.effective_launch_config(),
            LaunchMode::Explicit(LaunchInstanceConfig {
                availability_domain: "AD-1".to_string(),
                compartment_id: "ocid1.compartment.oc1..example".to_string(),
                subnet_id: "ocid1.subnet.oc1..example".to_string(),
                image_id: "ocid1.image.oc1..example".to_string(),
                display_name: "oci-cct-app-01".to_string(),
                ssh_authorized_keys: "ssh-rsa AAA...".to_string(),
                shape: Some("VM.Standard.A1.Flex".to_string()),
                shape_config: Some(ShapeConfig {
                    ocpus: 4,
                    memory_in_gbs: 24,
                }),
                boot_volume_size_in_gbs: None,
                boot_volume_vpus_per_gb: None,
                assign_public_ip: true,
                assign_private_dns_record: false,
                assign_ipv6_ip: false,
                ipv6_subnet_cidr: None,
            })
        );
    }

    #[test]
    fn uses_free_tier_defaults_when_launch_config_missing() {
        let config = AppConfig::load_from_path(write_temp_file("[oci]\n").as_path()).unwrap();

        match config.instance.effective_launch_config() {
            LaunchMode::FreeTierFallback(defaults) => {
                assert_eq!(defaults.shape_candidates.len(), 2);
                assert_eq!(defaults.shape_candidates[0].shape, "VM.Standard.A1.Flex");
            }
            LaunchMode::Explicit(_) => panic!("expected free tier fallback"),
        }
    }

    #[test]
    fn parses_oci_ini_profiles() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(
            &path,
            r#"
[DEFAULT]
user = ocid1.user.oc1..example
fingerprint = aa:bb:cc
tenancy = ocid1.tenancy.oc1..example
region = ap-chuncheon-1
key_file = /tmp/key.pem

[ALT]
user = second
fingerprint = dd:ee:ff
tenancy = tenancy
region = ap-seoul-1
key_file = /tmp/alt.pem
"#,
        )
        .unwrap();

        let config = OciIniConfig::load_from_path(&path).unwrap();
        assert_eq!(config.profiles.len(), 2);
        assert_eq!(config.profiles["DEFAULT"].region, "ap-chuncheon-1");
        assert_eq!(
            config.profiles["ALT"].key_file,
            PathBuf::from("/tmp/alt.pem")
        );
    }

    #[test]
    fn resolves_requested_profile_from_explicit_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(
            &path,
            r#"
[DEFAULT]
user = default-user
fingerprint = aa:bb:cc
tenancy = default-tenancy
region = ap-chuncheon-1
key_file = /tmp/default.pem

[CUSTOM]
user = custom-user
fingerprint = dd:ee:ff
tenancy = custom-tenancy
region = ap-seoul-1
key_file = /tmp/custom.pem
"#,
        )
        .unwrap();

        let credentials = OciProfileConfig {
            config_file: Some(path),
            profile: "CUSTOM".to_string(),
        }
        .resolve_credentials()
        .unwrap();

        assert_eq!(credentials.user, "custom-user");
        assert_eq!(credentials.region, "ap-seoul-1");
    }

    #[test]
    fn stores_and_reads_language_preferences() {
        let mut telegram = TelegramConfig::default();
        telegram.set_preferred_locale(42, "zh-TW");

        assert_eq!(telegram.preferred_locale(42, "en"), "zh-TW");
        assert_eq!(telegram.preferred_locale(7, "en"), "en");
    }

    fn write_temp_file(contents: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "oci-sniper-test-{}-{suffix}.toml",
            std::process::id()
        ));
        fs::write(&path, contents).unwrap();
        path
    }
}
