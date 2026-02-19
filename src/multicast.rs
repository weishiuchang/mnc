use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::os::fd::{AsRawFd, RawFd};

use nix::ifaddrs::getifaddrs;
use socket2::{Domain, Protocol, Socket, Type};

use crate::error::{LibError, Result};

pub fn create_recv_socket(iface: Option<&str>, mgroup: &str, port: u16) -> Result<Socket> {
    let mcast_addr: Ipv4Addr = mgroup.parse()?;

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;

    // Large receiver buffer (256MB) to handle higher packet rates
    set_recv_buffer_size(&socket, 256 * 1024 * 1024)?;

    // Let the kernel determin the default address if not specified by user
    let iface_addr = if let Some(iface_name) = iface {
        get_interface_addr(iface_name)?
    } else {
        get_default_interface_for_multicast(&mcast_addr)?
    };

    // IP_MULTICAST_IF
    socket.set_multicast_if_v4(&iface_addr)?;

    // IP_ADD_MEMBERSHIP
    // Join before bind to get the data flow going
    socket.join_multicast_v4(&mcast_addr, &iface_addr)?;

    let bind_addr = SocketAddr::new(IpAddr::V4(mcast_addr), port);
    socket.bind(&bind_addr.into())?;

    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;

    Ok(socket)
}

pub fn create_send_socket(iface: Option<&str>, mgroup: &str, port: u16, ttl: u8) -> Result<Socket> {
    let mcast_addr: Ipv4Addr = mgroup.parse()?;

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    if let Some(iface_name) = iface {
        let iface_addr = get_interface_addr(iface_name)?;
        socket.set_multicast_if_v4(&iface_addr)?;
    }

    // Useful troublehooting value for network engineers
    socket.set_multicast_ttl_v4(ttl.into())?;

    let dest_addr = SocketAddr::new(IpAddr::V4(mcast_addr), port);
    socket.connect(&dest_addr.into())?;

    socket.set_nonblocking(false)?;

    Ok(socket)
}

pub fn socket_to_raw_fd(socket: &Socket) -> RawFd {
    socket.as_raw_fd()
}

pub fn get_interface_addr(iface_name: &str) -> Result<Ipv4Addr> {
    for ifaddr in getifaddrs()? {
        if ifaddr.interface_name == iface_name
            && let Some(address) = ifaddr.address
            && let Some(sockaddr) = address.as_sockaddr_in()
        {
            let ip_bits = sockaddr.ip();
            return Ok(ip_bits);
        }
    }

    Err(LibError::Critical(format!(
        "Interface {iface_name} not found or has no IPv4 address"
    )))
}

pub fn get_default_interface_for_multicast(mcast_addr: &Ipv4Addr) -> Result<Ipv4Addr> {
    // Create a temporary UDP socket and connect to the multicast address.
    // The kernel will select the default route interface for us.
    let temp_socket = UdpSocket::bind("0.0.0.0:0")?;
    temp_socket.connect((*mcast_addr, 1))?;

    let local_addr = temp_socket.local_addr()?;

    match local_addr.ip() {
        IpAddr::V4(ipv4) => Ok(ipv4),
        IpAddr::V6(_) => Err(LibError::Critical(
            "IPv6 is not currently supported".to_string(),
        )),
    }
}

pub fn set_recv_buffer_size(socket: &Socket, size: usize) -> Result<()> {
    socket
        .set_recv_buffer_size(size)
        .map_err(|e| LibError::Critical(format!("Failed to set SO_RCVBUF to {size}: {e:?}")))?;

    let actual_size = socket.recv_buffer_size().unwrap_or(0);
    log::debug!("Receive Buffer: requested={size}, actual={actual_size}");

    Ok(())
}
