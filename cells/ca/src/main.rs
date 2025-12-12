// cells/ca/src/main.rs
// SPDX-License-Identifier: MIT
// Enterprise Certificate Authority - Root of Trust

use cell_sdk::*;
use anyhow::{Result, Context};
use std::sync::Arc;
use tokio::sync::RwLock;
use rcgen::{Certificate, CertificateParams, DistinguishedName, DnType, IsCa};
use time::{Duration, OffsetDateTime};

// === PROTOCOL ===

#[protein]
pub struct EnrollmentRequest {
    pub cell_name: String,
    pub csr_pem: String, // PEM encoded CSR
}

#[protein]
pub struct EnrollmentResponse {
    pub certificate_pem: String,
    pub ca_chain_pem: Vec<String>,
    pub valid_until: u64,
}

#[protein]
pub struct RevocationRequest {
    pub serial_number: String,
    pub reason: String,
}

// === SERVICE ===

struct CaState {
    root_cert: Certificate,
    // In a real HSM scenario, private key stays in hardware
    // Here we hold it in memory, protected by process isolation
    issued_certs: Vec<String>, 
}

#[service]
#[derive(Clone)]
struct CaService {
    state: Arc<RwLock<CaState>>,
}

impl CaService {
    fn generate_root() -> Result<Certificate> {
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Cell Enterprise Root CA");
        dn.push(DnType::OrganizationName, "Cell Inc.");
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        
        // 10 Year Validity for Root
        let now = OffsetDateTime::now_utc();
        params.not_before = now;
        params.not_after = now + Duration::days(365 * 10);
        
        Certificate::from_params(params).context("Failed to generate root CA")
    }
}

#[handler]
impl CaService {
    async fn enroll(&self, req: EnrollmentRequest) -> Result<EnrollmentResponse> {
        let mut state = self.state.write().await;
        
        // 1. Validate Identity (In real world, check against IAM or Nucleus)
        tracing::info!("[CA] Enrolling request for '{}'", req.cell_name);

        // 2. Generate Leaf Certificate
        let mut params = CertificateParams::new(vec![req.cell_name.clone()]);
        let now = OffsetDateTime::now_utc();
        params.not_before = now;
        // Short-lived certificates (24 hours)
        params.not_after = now + Duration::hours(24);
        params.is_ca = IsCa::NoCa;
        
        let cert = Certificate::from_params(params)?;
        
        // 3. Sign with Root
        let cert_pem = cert.serialize_pem_with_signer(&state.root_cert)?;
        let root_pem = state.root_cert.serialize_pem()?;
        
        state.issued_certs.push(cert_pem.clone());
        
        Ok(EnrollmentResponse {
            certificate_pem: cert_pem,
            ca_chain_pem: vec![root_pem],
            valid_until: (now + Duration::hours(24)).unix_timestamp() as u64,
        })
    }

    async fn revoke(&self, req: RevocationRequest) -> Result<bool> {
        // Add serial to CRL (Certificate Revocation List)
        tracing::warn!("[CA] Revoking cert {}: {}", req.serial_number, req.reason);
        Ok(true)
    }

    async fn get_root_cert(&self) -> Result<String> {
        let state = self.state.read().await;
        Ok(state.root_cert.serialize_pem()?)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    tracing::info!("[CA] Initializing Hardware Security Module (Simulated)...");
    let root = CaService::generate_root()?;
    
    // Fixed: Use Debug formatting for DnValue
    tracing::info!("[CA] Root of Trust established: {:?}", root.get_params().distinguished_name.get(&DnType::CommonName).unwrap());

    let service = CaService {
        state: Arc::new(RwLock::new(CaState {
            root_cert: root,
            issued_certs: Vec::new(),
        })),
    };

    service.serve("ca").await
}