use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, DATE, HOST};
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{FreeTierDefaults, LaunchInstanceConfig, OciCredentials, ShapeConfig};

const OCI_API_VERSION: &str = "20160918";

#[derive(Debug, Clone)]
pub struct OciClient {
    credentials: OciCredentials,
    http: reqwest::Client,
}

impl OciClient {
    pub fn new(credentials: OciCredentials) -> Self {
        Self {
            credentials,
            http: reqwest::Client::new(),
        }
    }

    pub async fn test_auth(&self, compartment_id: &str) -> Result<()> {
        self.get(
            &format!("/instances?compartmentId={compartment_id}"),
            &[("compartmentId", compartment_id)],
        )
        .await
        .map(|_| ())
    }

    pub async fn get_availability_domains(
        &self,
        compartment_id: &str,
    ) -> Result<Vec<AvailabilityDomain>> {
        self.get_json(
            &format!("/availabilityDomains?compartmentId={compartment_id}"),
            &[("compartmentId", compartment_id)],
        )
        .await
    }

    pub async fn get_subnets(&self, compartment_id: &str) -> Result<Vec<Subnet>> {
        self.get_json(
            &format!("/subnets?compartmentId={compartment_id}"),
            &[("compartmentId", compartment_id)],
        )
        .await
    }

    pub async fn get_images(
        &self,
        compartment_id: &str,
        shape: &str,
        operating_system: &str,
    ) -> Result<Vec<ImageSummary>> {
        self.get_json(
            &format!(
                "/images?compartmentId={compartment_id}&shape={shape}&operatingSystem={operating_system}&sortBy=TIMECREATED&sortOrder=DESC"
            ),
            &[
                ("compartmentId", compartment_id),
                ("shape", shape),
                ("operatingSystem", operating_system),
                ("sortBy", "TIMECREATED"),
                ("sortOrder", "DESC"),
            ],
        )
        .await
    }

    pub async fn create_instance(
        &self,
        request: &CreateInstanceRequest,
    ) -> Result<CreateInstanceResponse> {
        self.post_json("/instances", request).await
    }

    async fn get_json<T>(&self, path: &str, query: &[(&str, &str)]) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = self.get(path, query).await?;
        response
            .json::<T>()
            .await
            .context("failed to decode OCI GET response")
    }

    async fn post_json<T, R>(&self, path: &str, body: &T) -> Result<R>
    where
        T: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        let body_bytes = serde_json::to_vec(body).context("failed to serialize OCI POST body")?;
        let url = format!(
            "https://iaas.{}.oraclecloud.com/{}{}",
            self.credentials.region, OCI_API_VERSION, path
        );
        let host = format!("iaas.{}.oraclecloud.com", self.credentials.region);
        let date = format_http_date();
        let content_sha = BASE64.encode(Sha256::digest(&body_bytes));
        let authorization = self.sign_request(
            "post",
            path,
            &host,
            &date,
            Some(body_bytes.len()),
            Some(&content_sha),
        )?;

        let response = self
            .http
            .post(&url)
            .header(HOST, host)
            .header(DATE, date)
            .header(CONTENT_TYPE, "application/json")
            .header(CONTENT_LENGTH, body_bytes.len())
            .header("x-content-sha256", content_sha)
            .header(AUTHORIZATION, authorization)
            .body(body_bytes)
            .send()
            .await
            .context("failed to execute OCI POST request")?;

        handle_response(response).await
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Response> {
        let url = reqwest::Url::parse_with_params(
            &format!(
                "https://iaas.{}.oraclecloud.com/{}{}",
                self.credentials.region, OCI_API_VERSION, path
            ),
            query,
        )
        .context("failed to build OCI GET URL")?;
        let host = format!("iaas.{}.oraclecloud.com", self.credentials.region);
        let date = format_http_date();
        let request_target = canonical_get_target(path, query);
        let authorization = self.sign_request("get", &request_target, &host, &date, None, None)?;

        self.http
            .get(url)
            .header(HOST, host)
            .header(DATE, date)
            .header(AUTHORIZATION, authorization)
            .send()
            .await
            .context("failed to execute OCI GET request")
    }

    fn sign_request(
        &self,
        method: &str,
        request_target: &str,
        host: &str,
        date: &str,
        content_length: Option<usize>,
        content_sha: Option<&str>,
    ) -> Result<String> {
        let key = load_private_key(&self.credentials.key_file)?;

        let mut headers = vec!["(request-target)", "host", "date"];
        let mut signing_lines = vec![
            format!("(request-target): {method} {request_target}"),
            format!("host: {host}"),
            format!("date: {date}"),
        ];

        if let Some(content_sha) = content_sha {
            headers.extend(["x-content-sha256", "content-type", "content-length"]);
            signing_lines.push(format!("x-content-sha256: {content_sha}"));
            signing_lines.push("content-type: application/json".to_string());
            signing_lines.push(format!(
                "content-length: {}",
                content_length.unwrap_or_default()
            ));
        }

        let signing_string = signing_lines.join("\n");
        let signing_key = SigningKey::<Sha256>::new(key);
        let signature = signing_key.sign(signing_string.as_bytes());
        let signature_b64 = BASE64.encode(signature.to_bytes());
        let key_id = format!(
            "{}/{}/{}",
            self.credentials.tenancy, self.credentials.user, self.credentials.fingerprint
        );

        Ok(format!(
            "Signature version=\"1\",keyId=\"{key_id}\",algorithm=\"rsa-sha256\",headers=\"{}\",signature=\"{signature_b64}\"",
            headers.join(" ")
        ))
    }
}

#[derive(Debug, Clone)]
pub struct LaunchPlanner {
    defaults: FreeTierDefaults,
}

impl LaunchPlanner {
    pub fn new(defaults: FreeTierDefaults) -> Self {
        Self { defaults }
    }

    pub async fn resolve_defaults(&self, client: &OciClient) -> Result<ResolvedLaunch> {
        let compartment_id = client.credentials.tenancy.clone();
        let availability_domain = client
            .get_availability_domains(&compartment_id)
            .await?
            .into_iter()
            .next()
            .map(|ad| ad.name)
            .context("no OCI availability domain found")?;

        let subnet = client
            .get_subnets(&compartment_id)
            .await?
            .into_iter()
            .find(|subnet| subnet.lifecycle_state == "AVAILABLE")
            .context("no available subnet found in tenancy compartment")?;

        let shape = self
            .defaults
            .shape_candidates
            .first()
            .cloned()
            .context("no free-tier shape candidates configured")?;
        let image = self
            .discover_image(client, &compartment_id, &shape.shape)
            .await?;
        let ssh_authorized_keys = read_default_ssh_key()
            .context("failed to resolve default SSH public key for free-tier launch")?;

        let launch_config = LaunchInstanceConfig {
            availability_domain,
            compartment_id,
            subnet_id: subnet.id,
            image_id: image.id,
            display_name: "oci-sniper-free-tier".to_string(),
            ssh_authorized_keys,
            shape: Some(shape.shape),
            shape_config: Some(ShapeConfig {
                ocpus: shape.ocpus,
                memory_in_gbs: shape.memory_in_gbs,
            }),
            boot_volume_size_in_gbs: Some(200),
            boot_volume_vpus_per_gb: Some(120),
            assign_public_ip: self.defaults.assign_public_ip,
            assign_private_dns_record: true,
            assign_ipv6_ip: false,
            ipv6_subnet_cidr: None,
        };

        Ok(ResolvedLaunch {
            strategy: LaunchStrategy::FreeTierFallback,
            launch_config,
            selected_shape_ocpus: Some(shape.ocpus),
            selected_shape_memory_in_gbs: Some(shape.memory_in_gbs),
        })
    }

    async fn discover_image(
        &self,
        client: &OciClient,
        compartment_id: &str,
        shape: &str,
    ) -> Result<ImageSummary> {
        let images = client
            .get_images(compartment_id, shape, "Oracle Linux")
            .await?;
        images
            .into_iter()
            .find(|image| image.lifecycle_state == "AVAILABLE")
            .context("no Oracle Linux image found for selected shape")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLaunch {
    pub strategy: LaunchStrategy,
    pub launch_config: LaunchInstanceConfig,
    pub selected_shape_ocpus: Option<u32>,
    pub selected_shape_memory_in_gbs: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchStrategy {
    ExplicitConfig,
    FreeTierFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateInstanceRequest {
    pub availability_domain: String,
    pub compartment_id: String,
    pub metadata: Metadata,
    pub display_name: String,
    pub source_details: SourceDetails,
    pub shape: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape_config: Option<ShapeConfigPayload>,
    pub create_vnic_details: CreateVnicDetails,
    pub instance_options: InstanceOptions,
    pub defined_tags: serde_json::Value,
    pub freeform_tags: serde_json::Value,
    pub availability_config: AvailabilityConfig,
    pub agent_config: AgentConfig,
}

impl CreateInstanceRequest {
    pub fn from_launch_config(config: &LaunchInstanceConfig) -> Result<Self> {
        let shape = config
            .shape
            .clone()
            .or_else(|| {
                config
                    .shape_config
                    .as_ref()
                    .map(|_| "VM.Standard.A1.Flex".to_string())
            })
            .context("shape must be provided when building an OCI launch request")?;

        Ok(Self {
            availability_domain: config.availability_domain.clone(),
            compartment_id: config.compartment_id.clone(),
            metadata: Metadata {
                ssh_authorized_keys: config.ssh_authorized_keys.clone(),
            },
            display_name: config.display_name.clone(),
            source_details: SourceDetails {
                source_type: "image".to_string(),
                image_id: config.image_id.clone(),
                boot_volume_size_in_gbs: config.boot_volume_size_in_gbs.unwrap_or(200),
                boot_volume_vpus_per_gb: config.boot_volume_vpus_per_gb.unwrap_or(120),
            },
            shape,
            shape_config: config
                .shape_config
                .as_ref()
                .map(|shape| ShapeConfigPayload {
                    ocpus: shape.ocpus,
                    memory_in_gbs: shape.memory_in_gbs,
                }),
            create_vnic_details: CreateVnicDetails {
                assign_public_ip: config.assign_public_ip,
                subnet_id: config.subnet_id.clone(),
                assign_private_dns_record: config.assign_private_dns_record,
                assign_ipv6_ip: config.assign_ipv6_ip,
                ipv6_address_ipv6_subnet_cidr_pair_details: config
                    .ipv6_subnet_cidr
                    .as_ref()
                    .map(|cidr| {
                        vec![Ipv6AddressIpv6SubnetCidrPairDetail {
                            ipv6_subnet_cidr: cidr.clone(),
                        }]
                    })
                    .unwrap_or_default(),
            },
            instance_options: InstanceOptions {
                are_legacy_imds_endpoints_disabled: false,
            },
            defined_tags: serde_json::json!({}),
            freeform_tags: serde_json::json!({}),
            availability_config: AvailabilityConfig {
                recovery_action: "RESTORE_INSTANCE".to_string(),
            },
            agent_config: AgentConfig::default(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub ssh_authorized_keys: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceDetails {
    pub source_type: String,
    pub image_id: String,
    pub boot_volume_size_in_gbs: u32,
    pub boot_volume_vpus_per_gb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShapeConfigPayload {
    pub ocpus: u32,
    pub memory_in_gbs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateVnicDetails {
    pub assign_public_ip: bool,
    pub subnet_id: String,
    pub assign_private_dns_record: bool,
    pub assign_ipv6_ip: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ipv6_address_ipv6_subnet_cidr_pair_details: Vec<Ipv6AddressIpv6SubnetCidrPairDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Ipv6AddressIpv6SubnetCidrPairDetail {
    pub ipv6_subnet_cidr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceOptions {
    pub are_legacy_imds_endpoints_disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityConfig {
    pub recovery_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub plugins_config: Vec<PluginConfig>,
    pub is_monitoring_disabled: bool,
    pub is_management_disabled: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            plugins_config: vec![
                plugin("Custom Logs Monitoring", "ENABLED"),
                plugin("Compute Instance Run Command", "ENABLED"),
                plugin("Compute Instance Monitoring", "ENABLED"),
                plugin("Cloud Guard Workload Protection", "ENABLED"),
            ],
            is_monitoring_disabled: false,
            is_management_disabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfig {
    pub name: String,
    pub desired_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateInstanceResponse {
    pub id: String,
    pub lifecycle_state: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityDomain {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Subnet {
    pub id: String,
    pub lifecycle_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageSummary {
    pub id: String,
    pub lifecycle_state: String,
}

fn plugin(name: &str, desired_state: &str) -> PluginConfig {
    PluginConfig {
        name: name.to_string(),
        desired_state: desired_state.to_string(),
    }
}

async fn handle_response<R>(response: reqwest::Response) -> Result<R>
where
    R: for<'de> Deserialize<'de>,
{
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read OCI response body")?;
    if !status.is_success() {
        bail!("OCI request failed with status {status}: {body}");
    }

    serde_json::from_str(&body).context("failed to decode OCI response body")
}

fn load_private_key(path: &Path) -> Result<RsaPrivateKey> {
    let pem = fs::read_to_string(path)
        .with_context(|| format!("failed to read private key: {}", path.display()))?;
    if let Ok(key) = RsaPrivateKey::from_pkcs8_pem(&pem) {
        return Ok(key);
    }
    RsaPrivateKey::from_pkcs1_pem(&pem)
        .with_context(|| format!("failed to parse private key: {}", path.display()))
}

fn canonical_get_target(path: &str, query: &[(&str, &str)]) -> String {
    if query.is_empty() {
        return path.to_string();
    }

    let query = query
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

fn format_http_date() -> String {
    Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn read_default_ssh_key() -> Result<String> {
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    let candidates = [
        home.join(".ssh/id_ed25519.pub"),
        home.join(".ssh/id_rsa.pub"),
    ];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .map(|path| {
            fs::read_to_string(&path)
                .with_context(|| format!("failed to read SSH public key: {}", path.display()))
                .map(|value| value.trim().to_string())
        })
        .transpose()?
        .context("no default SSH public key found in ~/.ssh")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DefaultShapeCandidate, OciCredentials};
    use rand::thread_rng;
    use rsa::pkcs8::EncodePrivateKey;

    #[test]
    fn builds_create_instance_request_from_launch_config() {
        let request = CreateInstanceRequest::from_launch_config(&LaunchInstanceConfig {
            availability_domain: "AD-1".to_string(),
            compartment_id: "ocid1.compartment.oc1..example".to_string(),
            subnet_id: "ocid1.subnet.oc1..example".to_string(),
            image_id: "ocid1.image.oc1..example".to_string(),
            display_name: "oci-cct-app-01".to_string(),
            ssh_authorized_keys: "ssh-rsa AAA".to_string(),
            shape: Some("VM.Standard.A1.Flex".to_string()),
            shape_config: Some(ShapeConfig {
                ocpus: 4,
                memory_in_gbs: 24,
            }),
            boot_volume_size_in_gbs: Some(200),
            boot_volume_vpus_per_gb: Some(120),
            assign_public_ip: true,
            assign_private_dns_record: true,
            assign_ipv6_ip: true,
            ipv6_subnet_cidr: Some("2603:c024:0017:e000::/64".to_string()),
        })
        .unwrap();

        assert_eq!(request.shape, "VM.Standard.A1.Flex");
        assert_eq!(request.shape_config.unwrap().memory_in_gbs, 24);
        assert!(request.create_vnic_details.assign_ipv6_ip);
        assert_eq!(
            request
                .create_vnic_details
                .ipv6_address_ipv6_subnet_cidr_pair_details
                .len(),
            1
        );
    }

    #[test]
    fn keeps_free_tier_candidate_priority() {
        let planner = LaunchPlanner::new(FreeTierDefaults {
            shape_candidates: vec![
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
            ],
            assign_public_ip: true,
        });

        assert_eq!(
            planner.defaults.shape_candidates[0].shape,
            "VM.Standard.A1.Flex"
        );
    }

    #[test]
    fn canonicalizes_get_request_target() {
        assert_eq!(
            canonical_get_target("/instances", &[("compartmentId", "abc"), ("shape", "A1")]),
            "/instances?compartmentId=abc&shape=A1"
        );
    }

    #[test]
    fn signs_get_request_with_required_headers() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.pem");
        let private_key = RsaPrivateKey::new(&mut thread_rng(), 2048).unwrap();
        let pem = private_key.to_pkcs8_pem(Default::default()).unwrap();
        fs::write(&key_path, pem.as_bytes()).unwrap();
        let client = OciClient::new(OciCredentials {
            user: "ocid1.user.oc1..example".to_string(),
            fingerprint: "aa:bb:cc".to_string(),
            tenancy: "ocid1.tenancy.oc1..example".to_string(),
            region: "ap-chuncheon-1".to_string(),
            key_file: key_path,
        });

        let authorization = client
            .sign_request(
                "get",
                "/instances?compartmentId=ocid1.tenancy.oc1..example",
                "iaas.ap-chuncheon-1.oraclecloud.com",
                "Fri, 10 Apr 2026 03:50:52 GMT",
                None,
                None,
            )
            .unwrap();

        assert!(authorization.contains("algorithm=\"rsa-sha256\""));
        assert!(authorization.contains("headers=\"(request-target) host date\""));
        assert!(authorization.contains("signature=\""));
    }
}
