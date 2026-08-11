use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use ureq::Agent;

use crate::config::{Config, Credentials};
use crate::inventory::{Bastion, Inventory, Resource, ResourceKind};

const API_BASE: &str = "https://api.scaleway.com";
const PAGE_SIZE: usize = 100;

pub struct Client {
    agent: Agent,
    secret_key: String,
}

impl Client {
    pub fn new(credentials: &Credentials) -> Self {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .https_only(true)
            .build()
            .into();
        Self {
            agent,
            secret_key: credentials.secret_key.clone(),
        }
    }

    pub fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ureq::Error> {
        let mut request = self
            .agent
            .get(format!("{API_BASE}{path}"))
            .header("X-Auth-Token", &self.secret_key);
        for (key, value) in query {
            request = request.query(*key, value);
        }
        request.call()?.body_mut().read_json()
    }

    fn get_paged<L: PageList>(
        &self,
        path: &str,
        size_param: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<L::Item>, ureq::Error> {
        let mut items = Vec::new();
        for page in 1.. {
            let mut page_query = vec![
                (size_param, PAGE_SIZE.to_string()),
                ("page", page.to_string()),
            ];
            page_query.extend(query.iter().map(|(k, v)| (*k, v.clone())));
            let list: L = self.get_json(path, &page_query)?;
            let page_items = list.items();
            let page_len = page_items.len();
            items.extend(page_items);
            if page_len < PAGE_SIZE {
                break;
            }
        }
        Ok(items)
    }
}

trait PageList: DeserializeOwned {
    type Item;
    fn items(self) -> Vec<Self::Item>;
}

macro_rules! page_list {
    ($list:ty, $field:ident, $item:ty) => {
        impl PageList for $list {
            type Item = $item;
            fn items(self) -> Vec<$item> {
                self.$field
            }
        }
    };
}

#[derive(Debug, Deserialize)]
struct InstanceList {
    servers: Vec<Instance>,
}
page_list!(InstanceList, servers, Instance);

#[derive(Debug, Deserialize)]
struct Instance {
    id: String,
    name: String,
    state: String,
    #[serde(default)]
    tags: Vec<String>,
}

impl Instance {
    fn into_resource(self, zone: &str) -> Option<Resource> {
        (self.state == "running").then(|| Resource {
            kind: ResourceKind::Instance,
            id: self.id,
            name: self.name,
            zone: zone.to_owned(),
            tags: self.tags,
            endpoint_ip: None,
            endpoint_port: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct BaremetalList {
    servers: Vec<Baremetal>,
}
page_list!(BaremetalList, servers, Baremetal);

#[derive(Debug, Deserialize)]
struct Baremetal {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    tags: Vec<String>,
}

impl Baremetal {
    fn into_resource(self, zone: &str) -> Option<Resource> {
        (self.status == "ready").then(|| Resource {
            kind: ResourceKind::Baremetal,
            id: self.id,
            name: self.name,
            zone: zone.to_owned(),
            tags: self.tags,
            endpoint_ip: None,
            endpoint_port: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RdbList {
    instances: Vec<RdbInstance>,
}
page_list!(RdbList, instances, RdbInstance);

#[derive(Debug, Deserialize)]
struct RdbInstance {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    endpoints: Vec<RdbEndpoint>,
}

#[derive(Debug, Deserialize)]
struct RdbEndpoint {
    ip: Option<String>,
    hostname: Option<String>,
    port: u16,
}

impl RdbInstance {
    fn into_resource(self, region: &str) -> Option<Resource> {
        if self.status != "ready" {
            return None;
        }
        let endpoint = self.endpoints.into_iter().next();
        Some(Resource {
            kind: ResourceKind::Rdb,
            id: self.id,
            name: self.name,
            zone: region.to_owned(),
            tags: self.tags,
            endpoint_ip: endpoint
                .as_ref()
                .and_then(|e| e.ip.clone().or_else(|| e.hostname.clone())),
            endpoint_port: endpoint.map(|e| e.port),
        })
    }
}

#[derive(Debug, Deserialize)]
struct RedisList {
    clusters: Vec<RedisCluster>,
}
page_list!(RedisList, clusters, RedisCluster);

#[derive(Debug, Deserialize)]
struct RedisCluster {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    endpoints: Vec<RedisEndpoint>,
}

#[derive(Debug, Deserialize)]
struct RedisEndpoint {
    #[serde(default)]
    ips: Vec<String>,
    port: u16,
}

impl RedisCluster {
    fn into_resource(self, zone: &str) -> Option<Resource> {
        if self.status != "ready" {
            return None;
        }
        let endpoint = self.endpoints.into_iter().next();
        Some(Resource {
            kind: ResourceKind::Redis,
            id: self.id,
            name: self.name,
            zone: zone.to_owned(),
            tags: self.tags,
            endpoint_ip: endpoint.as_ref().and_then(|e| e.ips.first().cloned()),
            endpoint_port: endpoint.map(|e| e.port),
        })
    }
}

#[derive(Debug, Deserialize)]
struct LbList {
    lbs: Vec<Lb>,
}
page_list!(LbList, lbs, Lb);

#[derive(Debug, Deserialize)]
struct Lb {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    ip: Vec<LbIp>,
}

#[derive(Debug, Deserialize)]
struct LbIp {
    ip_address: String,
}

impl Lb {
    fn into_resource(self, zone: &str) -> Option<Resource> {
        if self.status != "ready" {
            return None;
        }
        let ip = self.ip.first().map(|ip| ip.ip_address.clone());
        Some(Resource {
            kind: ResourceKind::Lb,
            id: self.id,
            name: self.name,
            zone: zone.to_owned(),
            tags: self.tags,
            endpoint_ip: ip,
            endpoint_port: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct GatewayList {
    gateways: Vec<Gateway>,
}
page_list!(GatewayList, gateways, Gateway);

#[derive(Debug, Deserialize)]
struct Gateway {
    bastion_enabled: bool,
    bastion_port: u16,
    ipv4: Option<GatewayIp>,
}

#[derive(Debug, Deserialize)]
struct GatewayIp {
    address: String,
}

#[derive(Debug, Deserialize)]
struct IpamList {
    ips: Vec<IpamIp>,
}
page_list!(IpamList, ips, IpamIp);

#[derive(Debug, Deserialize)]
struct IpamIp {
    address: String,
    is_ipv6: bool,
    resource: Option<IpamResource>,
}

#[derive(Debug, Deserialize)]
struct IpamResource {
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
}

fn baremetal_private_ips(ips: Vec<IpamIp>) -> Vec<(String, String)> {
    ips.into_iter()
        .filter(|ip| !ip.is_ipv6)
        .filter_map(|ip| {
            let resource = ip.resource?;
            if resource.kind != "baremetal_private_nic" {
                return None;
            }
            let name = resource.name?;
            let address = ip.address.split('/').next()?.to_owned();
            Some((name, address))
        })
        .collect()
}

impl Client {
    fn list_instances(&self, zone: &str) -> Result<Vec<Resource>, ureq::Error> {
        let servers = self.get_paged::<InstanceList>(
            &format!("/instance/v1/zones/{zone}/servers"),
            "per_page",
            &[],
        )?;
        Ok(servers
            .into_iter()
            .filter_map(|s| s.into_resource(zone))
            .collect())
    }

    fn list_baremetal(&self, zone: &str) -> Result<Vec<Resource>, ureq::Error> {
        let servers = self.get_paged::<BaremetalList>(
            &format!("/baremetal/v1/zones/{zone}/servers"),
            "page_size",
            &[],
        )?;
        Ok(servers
            .into_iter()
            .filter_map(|s| s.into_resource(zone))
            .collect())
    }

    fn list_redis(&self, zone: &str) -> Result<Vec<Resource>, ureq::Error> {
        let clusters = self.get_paged::<RedisList>(
            &format!("/redis/v1/zones/{zone}/clusters"),
            "page_size",
            &[],
        )?;
        Ok(clusters
            .into_iter()
            .filter_map(|c| c.into_resource(zone))
            .collect())
    }

    fn list_lbs(&self, zone: &str) -> Result<Vec<Resource>, ureq::Error> {
        let lbs =
            self.get_paged::<LbList>(&format!("/lb/v1/zones/{zone}/lbs"), "page_size", &[])?;
        Ok(lbs
            .into_iter()
            .filter_map(|lb| lb.into_resource(zone))
            .collect())
    }

    fn list_rdb(&self, region: &str) -> Result<Vec<Resource>, ureq::Error> {
        let instances = self.get_paged::<RdbList>(
            &format!("/rdb/v1/regions/{region}/instances"),
            "page_size",
            &[],
        )?;
        Ok(instances
            .into_iter()
            .filter_map(|i| i.into_resource(region))
            .collect())
    }

    fn find_bastion(&self, zone: &str) -> Result<Option<Bastion>, ureq::Error> {
        let gateways = self.get_paged::<GatewayList>(
            &format!("/vpc-gw/v2/zones/{zone}/gateways"),
            "page_size",
            &[],
        )?;
        Ok(gateways
            .into_iter()
            .filter(|gateway| gateway.bastion_enabled)
            .find_map(|gateway| {
                let ip = gateway.ipv4?;
                Some(Bastion {
                    ip: ip.address,
                    port: gateway.bastion_port,
                    zone: zone.to_owned(),
                })
            }))
    }

    fn list_baremetal_private_ips(
        &self,
        region: &str,
    ) -> Result<Vec<(String, String)>, ureq::Error> {
        let ips = self.get_paged::<IpamList>(
            &format!("/ipam/v1/regions/{region}/ips"),
            "page_size",
            &[],
        )?;
        Ok(baremetal_private_ips(ips))
    }
}

fn is_auth_error(error: &ureq::Error) -> bool {
    matches!(error, ureq::Error::StatusCode(401 | 403))
}

fn collect_resources(
    tasks: Vec<(String, Result<Vec<Resource>, ureq::Error>)>,
) -> Result<Vec<Resource>> {
    let mut resources = Vec::new();
    for (label, result) in tasks {
        match result {
            Ok(mut items) => resources.append(&mut items),
            Err(error) if is_auth_error(&error) => {
                return Err(error).with_context(|| format!("scaleway denied access ({label})"));
            }
            // 501: the product does not exist in this zone.
            Err(ureq::Error::StatusCode(501)) => {}
            Err(error) => eprintln!("warning: skipping {label}: {error}"),
        }
    }
    Ok(resources)
}

pub fn fetch_inventory(credentials: &Credentials, config: &Config) -> Result<Inventory> {
    let client = Client::new(credentials);
    let zones = &config.scaleway.zones;
    let regions = &config.scaleway.regions;
    if zones.is_empty() || regions.is_empty() {
        bail!("config must declare at least one scaleway zone and region");
    }

    thread::scope(|scope| {
        let mut resource_tasks = Vec::new();
        for zone in zones {
            resource_tasks.push((
                format!("instances in {zone}"),
                scope.spawn(|| client.list_instances(zone)),
            ));
            resource_tasks.push((
                format!("baremetal in {zone}"),
                scope.spawn(|| client.list_baremetal(zone)),
            ));
            resource_tasks.push((
                format!("redis in {zone}"),
                scope.spawn(|| client.list_redis(zone)),
            ));
            resource_tasks.push((
                format!("load balancers in {zone}"),
                scope.spawn(|| client.list_lbs(zone)),
            ));
        }
        for region in regions {
            resource_tasks.push((
                format!("databases in {region}"),
                scope.spawn(|| client.list_rdb(region)),
            ));
        }
        let bastion_tasks: Vec<_> = zones
            .iter()
            .map(|zone| scope.spawn(|| client.find_bastion(zone)))
            .collect();
        let ipam_tasks: Vec<_> = regions
            .iter()
            .map(|region| scope.spawn(|| client.list_baremetal_private_ips(region)))
            .collect();

        let joined = resource_tasks
            .into_iter()
            .map(|(label, handle)| (label, handle.join().expect("api task panicked")))
            .collect();
        let mut resources = collect_resources(joined)?;

        let bastion = bastion_tasks
            .into_iter()
            .filter_map(|handle| handle.join().expect("api task panicked").ok().flatten())
            .next();

        let private_ips: Vec<(String, String)> = ipam_tasks
            .into_iter()
            .filter_map(|handle| handle.join().expect("api task panicked").ok())
            .flatten()
            .collect();
        for resource in &mut resources {
            if resource.kind == ResourceKind::Baremetal {
                resource.endpoint_ip = private_ips
                    .iter()
                    .find(|(name, _)| *name == resource.name)
                    .map(|(_, address)| address.clone());
            }
        }

        resources.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Inventory { resources, bastion })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_filter_on_running_state() {
        let list: InstanceList = serde_json::from_str(
            r#"{"servers": [
                {"id": "a", "name": "web-1", "state": "running", "tags": ["Env:Prod"]},
                {"id": "b", "name": "web-2", "state": "stopped", "tags": []}
            ]}"#,
        )
        .unwrap();

        let resources: Vec<_> = list
            .items()
            .into_iter()
            .filter_map(|s| s.into_resource("fr-par-1"))
            .collect();

        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].name, "web-1");
        assert_eq!(resources[0].zone, "fr-par-1");
        assert_eq!(resources[0].kind, ResourceKind::Instance);
    }

    #[test]
    fn rdb_takes_ip_or_hostname_from_the_first_endpoint() {
        let list: RdbList = serde_json::from_str(
            r#"{"instances": [
                {"id": "a", "name": "db-1", "status": "ready", "tags": [],
                 "endpoints": [{"ip": "10.0.0.5", "hostname": null, "port": 5432}]},
                {"id": "b", "name": "db-2", "status": "ready", "tags": [],
                 "endpoints": [{"ip": null, "hostname": "db-2.rdb.fr-par.scw.cloud", "port": 3306}]},
                {"id": "c", "name": "db-3", "status": "provisioning", "tags": [], "endpoints": []}
            ]}"#,
        )
        .unwrap();

        let resources: Vec<_> = list
            .items()
            .into_iter()
            .filter_map(|i| i.into_resource("fr-par"))
            .collect();

        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].endpoint_ip.as_deref(), Some("10.0.0.5"));
        assert_eq!(resources[0].endpoint_port, Some(5432));
        assert_eq!(
            resources[1].endpoint_ip.as_deref(),
            Some("db-2.rdb.fr-par.scw.cloud")
        );
    }

    #[test]
    fn redis_takes_the_first_endpoint_ip() {
        let list: RedisList = serde_json::from_str(
            r#"{"clusters": [
                {"id": "a", "name": "cache-1", "status": "ready", "tags": [],
                 "endpoints": [{"ips": ["172.16.4.2"], "port": 6379}]}
            ]}"#,
        )
        .unwrap();

        let resources: Vec<_> = list
            .items()
            .into_iter()
            .filter_map(|c| c.into_resource("fr-par-1"))
            .collect();

        assert_eq!(resources[0].endpoint_ip.as_deref(), Some("172.16.4.2"));
        assert_eq!(resources[0].endpoint_port, Some(6379));
    }

    #[test]
    fn lb_takes_the_first_ip_address() {
        let list: LbList = serde_json::from_str(
            r#"{"lbs": [
                {"id": "a", "name": "lb-1", "status": "ready", "tags": [],
                 "ip": [{"ip_address": "51.15.1.2"}]}
            ]}"#,
        )
        .unwrap();

        let resources: Vec<_> = list
            .items()
            .into_iter()
            .filter_map(|lb| lb.into_resource("fr-par-1"))
            .collect();

        assert_eq!(resources[0].endpoint_ip.as_deref(), Some("51.15.1.2"));
    }

    #[test]
    fn ipam_keeps_ipv4_baremetal_nics_and_strips_the_cidr() {
        let list: IpamList = serde_json::from_str(
            r#"{"ips": [
                {"address": "172.16.8.11/22", "is_ipv6": false,
                 "resource": {"type": "baremetal_private_nic", "name": "db-master-1"}},
                {"address": "fd00::1/64", "is_ipv6": true,
                 "resource": {"type": "baremetal_private_nic", "name": "db-master-1"}},
                {"address": "172.16.8.12/22", "is_ipv6": false,
                 "resource": {"type": "instance_private_nic", "name": "web-1"}},
                {"address": "172.16.8.13/22", "is_ipv6": false, "resource": null}
            ]}"#,
        )
        .unwrap();

        let ips = baremetal_private_ips(list.items());
        assert_eq!(ips, [("db-master-1".to_owned(), "172.16.8.11".to_owned())]);
    }

    #[test]
    fn bastion_requires_flag_and_ipv4() {
        let list: GatewayList = serde_json::from_str(
            r#"{"gateways": [
                {"bastion_enabled": false, "bastion_port": 61000, "ipv4": {"address": "1.2.3.4"}},
                {"bastion_enabled": true, "bastion_port": 61000, "ipv4": null},
                {"bastion_enabled": true, "bastion_port": 61000, "ipv4": {"address": "5.6.7.8"}}
            ]}"#,
        )
        .unwrap();

        let bastion = list
            .items()
            .into_iter()
            .filter(|g| g.bastion_enabled)
            .find_map(|g| g.ipv4.map(|ip| (ip.address, g.bastion_port)));

        assert_eq!(bastion, Some(("5.6.7.8".to_owned(), 61000)));
    }
}
