//! Scaleway API client and the concurrent inventory fetch.

use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use ureq::Agent;

use crate::config::{Config, Credentials};
use crate::inventory::{Bastion, Inventory, Resource, ResourceKind};
use crate::sensitive::Sensitive;

const API_BASE: &str = "https://api.scaleway.com";
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 100;

#[derive(Debug)]
pub(crate) enum FetchError {
    Api(ureq::Error),
    TooManyPages,
}

impl From<ureq::Error> for FetchError {
    fn from(error: ureq::Error) -> Self {
        Self::Api(error)
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(error) => error.fmt(f),
            Self::TooManyPages => write!(f, "gave up after {MAX_PAGES} pages"),
        }
    }
}

impl std::error::Error for FetchError {}

#[derive(Debug)]
pub(crate) struct Client {
    agent: Agent,
    secret_key: Sensitive,
}

impl Client {
    pub(crate) fn new(credentials: &Credentials) -> Self {
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

    pub(crate) fn fetch_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, ureq::Error> {
        let mut request = self
            .agent
            .get(format!("{API_BASE}{path}"))
            .header("X-Auth-Token", self.secret_key.expose());
        for (key, value) in query {
            request = request.query(*key, *value);
        }
        request.call()?.body_mut().read_json()
    }

    fn get_paged<L: PageList>(
        &self,
        path: &str,
        size_param: &str,
        query: &[(&str, &str)],
    ) -> Result<Vec<L::Item>, FetchError> {
        collect_pages(|page| {
            let page_size = PAGE_SIZE.to_string();
            let page = page.to_string();
            let mut page_query = vec![(size_param, page_size.as_str()), ("page", page.as_str())];
            page_query.extend_from_slice(query);
            let list: L = self.fetch_json(path, &page_query)?;
            Ok(list.into_parts())
        })
    }
}

/// Pages until the server-reported total is reached, an empty page arrives,
/// or (without a total) a short page. Capped: an endpoint that ignores the
/// page parameter must not hang or grow unboundedly.
fn collect_pages<T>(
    mut fetch_page: impl FnMut(usize) -> Result<(Vec<T>, Option<u64>), FetchError>,
) -> Result<Vec<T>, FetchError> {
    let mut items = Vec::new();
    for page in 1..=MAX_PAGES {
        let (page_items, total_count) = fetch_page(page)?;
        if page_items.is_empty() {
            return Ok(items);
        }
        let short_page = page_items.len() < PAGE_SIZE;
        items.extend(page_items);
        match total_count {
            Some(total) => {
                if items.len() as u64 >= total {
                    return Ok(items);
                }
            }
            None => {
                if short_page {
                    return Ok(items);
                }
            }
        }
    }
    Err(FetchError::TooManyPages)
}

trait PageList: DeserializeOwned {
    type Item;
    fn into_parts(self) -> (Vec<Self::Item>, Option<u64>);
}

macro_rules! page_list {
    ($list:ty, $field:ident, $item:ty) => {
        impl PageList for $list {
            type Item = $item;
            fn into_parts(self) -> (Vec<$item>, Option<u64>) {
                (self.$field, self.total_count)
            }
        }
    };
}

#[derive(Debug, Deserialize)]
struct InstanceList {
    #[serde(default)]
    total_count: Option<u64>,
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
    #[serde(default)]
    total_count: Option<u64>,
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
    #[serde(default)]
    total_count: Option<u64>,
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
    #[serde(default)]
    total_count: Option<u64>,
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
    #[serde(default)]
    total_count: Option<u64>,
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
    #[serde(default)]
    total_count: Option<u64>,
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
    #[serde(default)]
    total_count: Option<u64>,
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
    fn list_instances(&self, zone: &str) -> Result<Vec<Resource>, FetchError> {
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

    fn list_baremetal(&self, zone: &str) -> Result<Vec<Resource>, FetchError> {
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

    fn list_redis(&self, zone: &str) -> Result<Vec<Resource>, FetchError> {
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

    fn list_lbs(&self, zone: &str) -> Result<Vec<Resource>, FetchError> {
        let lbs =
            self.get_paged::<LbList>(&format!("/lb/v1/zones/{zone}/lbs"), "page_size", &[])?;
        Ok(lbs
            .into_iter()
            .filter_map(|lb| lb.into_resource(zone))
            .collect())
    }

    fn list_rdb(&self, region: &str) -> Result<Vec<Resource>, FetchError> {
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

    fn find_bastion(&self, zone: &str) -> Result<Option<Bastion>, FetchError> {
        let gateways = self.get_paged::<GatewayList>(
            &format!("/vpc-gw/v2/zones/{zone}/gateways"),
            "page_size",
            &[],
        )?;
        Ok(select_bastion(gateways, zone))
    }

    fn list_baremetal_private_ips(
        &self,
        region: &str,
    ) -> Result<Vec<(String, String)>, FetchError> {
        let ips = self.get_paged::<IpamList>(
            &format!("/ipam/v1/regions/{region}/ips"),
            "page_size",
            &[],
        )?;
        Ok(baremetal_private_ips(ips))
    }
}

fn select_bastion(gateways: Vec<Gateway>, zone: &str) -> Option<Bastion> {
    gateways
        .into_iter()
        .filter(|gateway| gateway.bastion_enabled)
        .find_map(|gateway| {
            let ip = gateway.ipv4?;
            Some(Bastion {
                ip: ip.address,
                port: gateway.bastion_port,
                zone: zone.to_owned(),
            })
        })
}

fn is_auth_error(error: &FetchError) -> bool {
    matches!(error, FetchError::Api(ureq::Error::StatusCode(401 | 403)))
}

fn collect_resources(
    tasks: Vec<(String, Result<Vec<Resource>, FetchError>)>,
) -> Result<(Vec<Resource>, bool)> {
    let mut resources = Vec::new();
    let mut complete = true;
    for (label, result) in tasks {
        match result {
            Ok(mut items) => resources.append(&mut items),
            Err(error) if is_auth_error(&error) => {
                return Err(error).with_context(|| format!("scaleway denied access ({label})"));
            }
            // 501: the product does not exist in this zone.
            Err(FetchError::Api(ureq::Error::StatusCode(501))) => {}
            Err(error) => {
                eprintln!("warning: skipping {label}: {error}");
                complete = false;
            }
        }
    }
    Ok((resources, complete))
}

/// A fetched inventory and whether every zone answered; partial results
/// must not be cached, or one network blip poisons every command for the
/// cache TTL.
pub(crate) struct Fetched {
    pub(crate) inventory: Inventory,
    pub(crate) complete: bool,
}

pub(crate) fn fetch_inventory(credentials: &Credentials, config: &Config) -> Result<Fetched> {
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
            .map(|zone| (zone, scope.spawn(|| client.find_bastion(zone))))
            .collect();
        let ipam_tasks: Vec<_> = regions
            .iter()
            .map(|region| {
                (
                    region,
                    scope.spawn(|| client.list_baremetal_private_ips(region)),
                )
            })
            .collect();

        let joined = resource_tasks
            .into_iter()
            .map(|(label, handle)| (label, handle.join().expect("api task panicked")))
            .collect();
        let (mut resources, mut complete) = collect_resources(joined)?;

        let mut bastion = None;
        for (zone, handle) in bastion_tasks {
            match handle.join().expect("api task panicked") {
                Ok(found) => {
                    if bastion.is_none() {
                        bastion = found;
                    }
                }
                Err(FetchError::Api(ureq::Error::StatusCode(501))) => {}
                Err(error) if is_auth_error(&error) => {
                    return Err(error)
                        .with_context(|| format!("scaleway denied access (gateways in {zone})"));
                }
                Err(error) => {
                    eprintln!("warning: skipping gateways in {zone}: {error}");
                    complete = false;
                }
            }
        }

        let mut private_ips: Vec<(String, String)> = Vec::new();
        for (region, handle) in ipam_tasks {
            match handle.join().expect("api task panicked") {
                Ok(mut ips) => private_ips.append(&mut ips),
                Err(FetchError::Api(ureq::Error::StatusCode(501))) => {}
                Err(error) if is_auth_error(&error) => {
                    return Err(error)
                        .with_context(|| format!("scaleway denied access (ipam in {region})"));
                }
                Err(error) => {
                    eprintln!("warning: skipping ipam in {region}: {error}");
                    complete = false;
                }
            }
        }
        for resource in &mut resources {
            if resource.kind == ResourceKind::Baremetal {
                resource.endpoint_ip = private_ips
                    .iter()
                    .find(|(name, _)| *name == resource.name)
                    .map(|(_, address)| address.clone());
            }
        }

        if resources.is_empty() && !complete {
            bail!("inventory fetch failed for every zone");
        }

        resources.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Fetched {
            inventory: Inventory::new(resources, bastion),
            complete,
        })
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
            .into_parts()
            .0
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
            .into_parts()
            .0
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
            .into_parts()
            .0
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
            .into_parts()
            .0
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

        let ips = baremetal_private_ips(list.into_parts().0);
        assert_eq!(ips, [("db-master-1".to_owned(), "172.16.8.11".to_owned())]);
    }

    fn resource(name: &str) -> Resource {
        Resource {
            kind: ResourceKind::Instance,
            id: "id".to_owned(),
            name: name.to_owned(),
            zone: "fr-par-1".to_owned(),
            tags: vec![],
            endpoint_ip: None,
            endpoint_port: None,
        }
    }

    #[test]
    fn collect_treats_absent_products_as_complete() {
        let (resources, complete) = collect_resources(vec![
            ("instances in fr-par-1".to_owned(), Ok(vec![resource("a")])),
            (
                "redis in fr-par-3".to_owned(),
                Err(FetchError::Api(ureq::Error::StatusCode(501))),
            ),
        ])
        .unwrap();

        assert_eq!(resources.len(), 1);
        assert!(complete);
    }

    #[test]
    fn collect_marks_transient_failures_incomplete() {
        let (resources, complete) = collect_resources(vec![
            ("instances in fr-par-1".to_owned(), Ok(vec![resource("a")])),
            (
                "baremetal in fr-par-2".to_owned(),
                Err(FetchError::Api(ureq::Error::StatusCode(503))),
            ),
        ])
        .unwrap();

        assert_eq!(resources.len(), 1);
        assert!(!complete);
    }

    #[test]
    fn collect_propagates_auth_errors() {
        let result = collect_resources(vec![(
            "instances in fr-par-1".to_owned(),
            Err(FetchError::Api(ureq::Error::StatusCode(401))),
        )]);
        assert!(result.is_err());
    }

    fn pages<T: Clone>(
        pages: Vec<(Vec<T>, Option<u64>)>,
    ) -> impl FnMut(usize) -> Result<(Vec<T>, Option<u64>), FetchError> {
        move |page| Ok(pages[page - 1].clone())
    }

    #[test]
    fn pagination_stops_on_the_reported_total() {
        let full: Vec<u32> = (0..100).collect();
        let items = collect_pages(pages(vec![
            (full.clone(), Some(150)),
            ((0..50).collect(), Some(150)),
        ]))
        .unwrap();
        assert_eq!(items.len(), 150);
    }

    #[test]
    fn pagination_without_total_stops_on_a_short_page() {
        let items = collect_pages(pages(vec![((0..30).collect::<Vec<u32>>(), None)])).unwrap();
        assert_eq!(items.len(), 30);
    }

    #[test]
    fn pagination_stops_on_an_empty_page() {
        let full: Vec<u32> = (0..100).collect();
        let items = collect_pages(pages(vec![(full, None), (vec![], None)])).unwrap();
        assert_eq!(items.len(), 100);
    }

    #[test]
    fn pagination_gives_up_when_the_server_ignores_the_page_parameter() {
        let full: Vec<u32> = (0..100).collect();
        let result = collect_pages(move |_| Ok((full.clone(), None)));
        assert!(matches!(result, Err(FetchError::TooManyPages)));
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

        let bastion = select_bastion(list.into_parts().0, "fr-par-1").unwrap();

        assert_eq!(bastion.ip, "5.6.7.8");
        assert_eq!(bastion.port, 61000);
        assert_eq!(bastion.zone, "fr-par-1");
    }
}
