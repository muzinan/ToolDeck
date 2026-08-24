//! DNS 查询领域模型。

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum DnsRecordType {
    #[default]
    Auto,
    A,
    Aaaa,
    Cname,
    Mx,
    Txt,
    Ns,
    Ptr,
}

impl DnsRecordType {
    pub const ALL: [Self; 8] = [
        Self::Auto,
        Self::A,
        Self::Aaaa,
        Self::Cname,
        Self::Mx,
        Self::Txt,
        Self::Ns,
        Self::Ptr,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::A => "A",
            Self::Aaaa => "AAAA",
            Self::Cname => "CNAME",
            Self::Mx => "MX",
            Self::Txt => "TXT",
            Self::Ns => "NS",
            Self::Ptr => "PTR",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DnsRecord {
    pub name: String,
    pub record_type: DnsRecordType,
    pub value: String,
    pub ttl: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DnsResult {
    pub host: String,
    pub records: Vec<DnsRecord>,
}
