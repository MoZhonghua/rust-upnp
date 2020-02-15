/*!
Search is a fundamental part of a control point's operation, typically a multicast request
is sent out periodically and devices on the network can respond directly to the control point
with their descriptions. With v1.1 of the SSDP specification a unicast search was added to
send a request to a specific device.

This module provides three functions that provide 1) multicast search, 2) unicast search, and 3)
multicast search with caching. The caching version of search will merge the set of new responses
with any (non-expired) previously cached responses.

*/
use crate::httpu::{
    multicast, Options as MulticastOptions, RequestBuilder, Response as MulticastResponse,
};
use crate::ssdp::{protocol, ControlPoint};
use crate::utils::{headers, user_agent};
use crate::{Error, MessageErrorKind, SpecVersion};
use regex::Regex;
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::fmt::{Display, Error as FmtError, Formatter};
use std::net::SocketAddrV4;
use std::str::FromStr;

// ------------------------------------------------------------------------------------------------
// Public Types
// ------------------------------------------------------------------------------------------------

///
/// `SearchTarget` corresponds to the set of values defined by the UDA `ST` header.
///
/// This type does not separate out the version of a device or service type, it does ensure
/// that the ':' separator character is present in the combined value.
///
#[derive(Clone, Debug)]
pub enum SearchTarget {
    /// Corresponds to the value `ssdp:all`
    All,
    /// Corresponds to the value `upnp:rootdevice`
    RootDevices,
    /// Corresponds to the value `uuid:{device-UUID}`
    Device(String),
    /// Corresponds to the value `urn:schemas-upnp-org:device:{deviceType:ver}`
    DeviceType(String),
    /// Corresponds to the value `urn:schemas-upnp-org:service:{serviceType:ver}`
    ServiceType(String),
    /// Corresponds to the value `urn:{domain-name}:device:{deviceType:ver}`
    DomainDeviceType(String, String),
    /// Corresponds to the value `urn:{domain-name}:service:{serviceType:ver}`
    DomainServiceType(String, String),
}

///
/// This type encapsulates a set of mostly optional values to be used to construct messages to
/// send.
///
/// As such `Options::default()` is usually sufficient, in cases where a client wishes to select
/// a specific version of the specification use `Options::new`. Currently the only time a value
/// is required is when the version is set to 2.0, a value **is** required for the control point.
/// The `Options::for_control_point` will set the control point as well as the version number.
///
#[derive(Clone, Debug)]
pub struct Options {
    /// The specification that will be used to construct sent messages and to verify responses.
    /// Default: `SpecVersion:V10`.
    pub spec_version: SpecVersion,
    /// The scope of the search to perform. Default: `SearchTarget::RootDevices`.
    pub search_target: SearchTarget,
    /// A specific network interface to bind to; if specified the default address for the interface
    /// will be used, else the address `0.0.0.0:0` will be used. Default: `None`.
    pub network_interface: Option<String>,
    /// The maximum wait time for devices to use in responding. This will also be used as the read
    /// timeout on the underlying socket. This value **must** be between `0` and `120`;
    /// default: `2`.
    pub max_wait_time: u8,
    /// If specified this is to be the `ProduceName/Version` component of the user agent string
    /// the client will generate as part of sent messages. If not specified a default value based
    /// on the name and version of this crate will be used. Default: `None`.
    pub product_and_version: Option<String>,
    /// If specified this will be used to add certain control point values in the sent messages.
    /// This value is **only** used by the 2.0 specification where it is required, otherwise it
    /// will be ignores. Default: `None`.
    pub control_point: Option<ControlPoint>,
}

#[derive(Clone, Debug)]
struct CachedResponse {
    response: Response,
    expiration: u64,
}

#[derive(Clone, Debug)]
pub struct ResponseCache {
    options: Options,
    minimum_refresh: u16,
    last_updated: u64,
    responses: Vec<CachedResponse>,
}

#[derive(Clone, Debug)]
pub struct Response {
    max_age: u64,
    date: String,
    server: String,
    location: String,
    search_target: SearchTarget,
    service_name: String,
    boot_id: u64,
    other_headers: HashMap<String, String>,
}

// ------------------------------------------------------------------------------------------------
// Public Functions
// ------------------------------------------------------------------------------------------------

///
/// Perform a multicast search but store the results in a cache that allows a client to keep
/// the results around and use the `update` method to refresh the cache from the network.
///
/// The search function can be configured using the [`Options`](struct.Options.html) struct,
/// although the defaults are reasonable for most clients.
///
pub fn search(options: Options) -> Result<ResponseCache, Error> {
    info!("search - options: {:?}", options);
    options.validate()?;
    Err(Error::MessageFormat(MessageErrorKind::VersionMismatch))
}

///
/// Perform a multicast search but return the results immediately as a vector, not wrapped
/// in a cache.
///
/// The search function can be configured using the [`Options`](struct.Options.html) struct,
/// although the defaults are reasonable for most clients.
///
pub fn search_once(options: Options) -> Result<Vec<Response>, Error> {
    info!("search_once - options: {:?}", options);
    options.validate()?;
    let mut message_builder = RequestBuilder::new(protocol::METHOD_SEARCH);
    // All headers from the original 1.0 specification.
    message_builder
        .add_header(protocol::HEAD_HOST, protocol::MULTICAST_ADDRESS)
        .add_header(protocol::HEAD_MAN, protocol::HTTP_EXTENSION)
        .add_header(protocol::HEAD_MX, &format!("{}", options.max_wait_time))
        .add_header(protocol::HEAD_ST, &options.search_target.to_string());
    // Headers added by 1.1 specification
    if options.spec_version >= SpecVersion::V11 {
        message_builder.add_header(
            protocol::HEAD_USER_AGENT,
            &user_agent::make(&options.spec_version, &options.product_and_version),
        );
    }
    // Headers added by 2.0 specification
    if options.spec_version >= SpecVersion::V20 {
        match &options.control_point {
            Some(cp) => {
                message_builder.add_header(protocol::HEAD_CP_FN, &cp.friendly_name);
                if let Some(port) = cp.port {
                    message_builder.add_header(protocol::HEAD_TCP_PORT, &port.to_string());
                }
                if let Some(uuid) = &cp.uuid {
                    message_builder.add_header(protocol::HEAD_TCP_PORT, &uuid);
                }
            }
            None => {
                error!("search_once - missing control point, required for UPnP/2.0");
                return Err(Error::MessageFormat(MessageErrorKind::MissingRequiredField));
            }
        }
    }
    trace!("search_once - {:?}", &message_builder);
    let raw_responses = multicast(
        &message_builder.into(),
        &protocol::MULTICAST_ADDRESS.parse().unwrap(),
        &options.into(),
    )?;

    let mut responses: Vec<Response> = Vec::new();
    for raw_response in raw_responses {
        responses.push(raw_response.try_into()?);
    }
    Ok(responses)
}

///
/// Perform a unicast search and return the results immediately as a vector, not wrapped
/// in a cache.
///
/// The search function can be configured using the [`Options`](struct.Options.html) struct,
/// although the defaults are reasonable for most clients.
///
pub fn search_once_to_device(
    options: Options,
    device_address: SocketAddrV4,
) -> Result<Vec<Response>, Error> {
    info!(
        "search_once_to_device - options: {:?}, device_address: {:?}",
        options, device_address
    );
    options.validate()?;
    if options.spec_version >= SpecVersion::V11 {
        let mut message_builder = RequestBuilder::new(protocol::METHOD_SEARCH);
        message_builder
            .add_header(protocol::HEAD_HOST, protocol::MULTICAST_ADDRESS)
            .add_header(protocol::HEAD_MAN, protocol::HTTP_EXTENSION)
            .add_header(protocol::HEAD_ST, &options.search_target.to_string())
            .add_header(
                protocol::HEAD_USER_AGENT,
                &user_agent::make(&options.spec_version, &options.product_and_version),
            );

        let raw_responses = multicast(&message_builder.into(), &device_address, &options.into())?;

        let mut responses: Vec<Response> = Vec::new();
        for raw_response in raw_responses {
            responses.push(raw_response.try_into()?);
        }
        Ok(responses)
    } else {
        Err(Error::Unsupported)
    }
}

// ------------------------------------------------------------------------------------------------
// Implementations
// ------------------------------------------------------------------------------------------------

impl Display for SearchTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), FmtError> {
        write!(
            f,
            "{}",
            match self {
                SearchTarget::All => "ssdp::all".to_string(),
                SearchTarget::RootDevices => "upnp:rootdevice".to_string(),
                SearchTarget::Device(device) => format!("uuid:{}", device),
                SearchTarget::DeviceType(device) =>
                    format!("urn:schemas-upnp-org:device:{}", device),
                SearchTarget::ServiceType(service) =>
                    format!("urn:schemas-upnp-org:service:{}", service),
                SearchTarget::DomainDeviceType(domain, device) =>
                    format!("urn:{}:device:{}", domain, device),
                SearchTarget::DomainServiceType(domain, service) =>
                    format!("urn:{}:service:{}", domain, service),
            }
        )
    }
}

impl FromStr for SearchTarget {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "ssdp::all" {
            Ok(SearchTarget::All)
        } else if s == "upnp:rootdevice" {
            Ok(SearchTarget::RootDevices)
        } else if s.starts_with("uuid:") {
            Ok(SearchTarget::Device(s[5..].to_string()))
        } else if s.starts_with("urn:schemas-upnp-org:device:") {
            Ok(SearchTarget::DeviceType(s[28..].to_string()))
        } else if s.starts_with("urn:schemas-upnp-org:service:") {
            Ok(SearchTarget::ServiceType(s[29..].to_string()))
        // TODO: domain devices and services
        } else {
            Err(())
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Options {
            spec_version: SpecVersion::V10,
            network_interface: None,
            search_target: SearchTarget::RootDevices,
            max_wait_time: 2,
            product_and_version: None,
            control_point: None,
        }
    }
}
impl Options {
    pub fn new(spec_version: SpecVersion) -> Self {
        let mut new = Self::default();
        new.spec_version = spec_version;
        new
    }

    pub fn for_control_point(control_point: ControlPoint) -> Self {
        let mut new = Self::default();
        new.spec_version = SpecVersion::V20;
        new.control_point = Some(control_point.clone());
        new
    }

    pub fn validate(&self) -> Result<(), Error> {
        lazy_static! {
            static ref RE: Regex = Regex::new(r"^$").unwrap();
        }
        if self.max_wait_time < 1 || self.max_wait_time > 120 {
            error!(
                "validate - max_wait_time must be between 1..120 ({})",
                self.max_wait_time
            );
            return Err(Error::MessageFormat(MessageErrorKind::InvalidFieldValue));
        }
        if self.spec_version >= SpecVersion::V11 {
            if let Some(user_agent) = &self.product_and_version {
                if !RE.is_match(user_agent) {
                    error!(
                        "validate - user_agent needs to match 'ProductName/Version' ({})",
                        user_agent
                    );
                    return Err(Error::MessageFormat(MessageErrorKind::InvalidFieldValue));
                }
            }
        }
        if self.spec_version >= SpecVersion::V20 {
            if self.control_point.is_none() {
                error!("validate - control_point required");
                return Err(Error::MessageFormat(MessageErrorKind::InvalidFieldValue));
            } else if let Some(control_point) = &self.control_point {
                if control_point.friendly_name.is_empty() {
                    error!("validate - control_point.friendly_name required");
                    return Err(Error::MessageFormat(MessageErrorKind::InvalidFieldValue));
                }
            }
        }
        Ok(())
    }
}

impl From<Options> for MulticastOptions {
    fn from(options: Options) -> Self {
        let mut multicast_options = MulticastOptions::default();
        multicast_options.network_interface = options.network_interface;
        multicast_options.timeout = options.max_wait_time as u64;
        multicast_options
    }
}

const REQUIRED_HEADERS: [&str; 7] = [
    protocol::HEAD_BOOTID,
    protocol::HEAD_CACHE_CONTROL,
    protocol::HEAD_DATE,
    protocol::HEAD_EXT,
    protocol::HEAD_LOCATION,
    protocol::HEAD_ST,
    protocol::HEAD_USN,
];

impl TryFrom<MulticastResponse> for Response {
    type Error = Error;

    fn try_from(response: MulticastResponse) -> Result<Self, Self::Error> {
        headers::check_required(&response.headers, &REQUIRED_HEADERS)?;
        headers::check_empty(
            response.headers.get(protocol::HEAD_EXT).unwrap(),
            protocol::HEAD_EXT,
        )?;

        let remaining_headers: HashMap<String, String> = response
            .headers
            .clone()
            .iter()
            .filter(|(k, _)| REQUIRED_HEADERS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Ok(Response {
            boot_id: headers::check_parsed_value::<u64>(
                response.headers.get(protocol::HEAD_BOOTID).unwrap(),
                protocol::HEAD_BOOTID,
            )?,
            max_age: headers::check_parsed_value::<u64>(
                &headers::check_regex(
                    response.headers.get(protocol::HEAD_CACHE_CONTROL).unwrap(),
                    protocol::HEAD_CACHE_CONTROL,
                    &Regex::new(r"max-age[ ]*=[ ]*(\d+)").unwrap(),
                )?,
                protocol::HEAD_CACHE_CONTROL,
            )?,
            date: headers::check_not_empty(
                response.headers.get(protocol::HEAD_DATE).unwrap(),
                protocol::HEAD_DATE,
            )?,
            server: headers::check_not_empty(
                response.headers.get(protocol::HEAD_SERVER).unwrap(),
                protocol::HEAD_SERVER,
            )?,
            location: headers::check_not_empty(
                response.headers.get(protocol::HEAD_LOCATION).unwrap(),
                protocol::HEAD_LOCATION,
            )?,
            search_target: SearchTarget::All,
            service_name: headers::check_not_empty(
                response.headers.get(protocol::HEAD_USN).unwrap(),
                protocol::HEAD_USN,
            )?,
            other_headers: remaining_headers,
        })
    }
}

impl ResponseCache {
    pub fn refresh(&mut self) -> Self {
        self.to_owned()
    }

    pub fn last_updated(self) -> u64 {
        self.last_updated
    }

    pub fn responses(&self) -> Vec<Response> {
        Vec::new()
    }
}
