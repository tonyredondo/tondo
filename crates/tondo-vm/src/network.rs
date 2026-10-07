//! Closed hosted networking identities. These descriptors carry no provider
//! state and do not establish a native ABI or initialize network resources.

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum NetworkType {
    NetLimits,
    NetOptions,
    TcpListener,
    TcpStream,
    TcpReadHalf,
    TcpWriteHalf,
    UdpSocket,
    Datagram,
    TlsConfig,
    TlsStream,
    TlsReadHalf,
    TlsWriteHalf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NetworkOperation {
    HostName,
    IpParse,
    SocketAddress,
    LimitsCreate,
    LimitsDefaults,
    Options,
    Resolve,
    Connect,
    Listen,
    ListenerAccept,
    ListenerLocal,
    ListenerClose,
    TcpSplit,
    TcpLocal,
    TcpPeer,
    TcpShutdown,
    TcpClose,
    TcpRead,
    TcpReadClose,
    TcpWrite,
    TcpFlush,
    TcpWriteShutdown,
    TcpWriteClose,
    Bind,
    UdpSend,
    UdpReceive,
    UdpLocal,
    UdpClose,
    DatagramBytes,
    DatagramSource,
    TlsConfig,
    TlsConnect,
    TlsSplit,
    TlsClose,
    TlsRead,
    TlsReadClose,
    TlsWrite,
    TlsFlush,
    TlsShutdown,
    TlsWriteClose,
}

impl NetworkOperation {
    pub const ALL: [Self; 40] = [
        Self::HostName,
        Self::IpParse,
        Self::SocketAddress,
        Self::LimitsCreate,
        Self::LimitsDefaults,
        Self::Options,
        Self::Resolve,
        Self::Connect,
        Self::Listen,
        Self::ListenerAccept,
        Self::ListenerLocal,
        Self::ListenerClose,
        Self::TcpSplit,
        Self::TcpLocal,
        Self::TcpPeer,
        Self::TcpShutdown,
        Self::TcpClose,
        Self::TcpRead,
        Self::TcpReadClose,
        Self::TcpWrite,
        Self::TcpFlush,
        Self::TcpWriteShutdown,
        Self::TcpWriteClose,
        Self::Bind,
        Self::UdpSend,
        Self::UdpReceive,
        Self::UdpLocal,
        Self::UdpClose,
        Self::DatagramBytes,
        Self::DatagramSource,
        Self::TlsConfig,
        Self::TlsConnect,
        Self::TlsSplit,
        Self::TlsClose,
        Self::TlsRead,
        Self::TlsReadClose,
        Self::TlsWrite,
        Self::TlsFlush,
        Self::TlsShutdown,
        Self::TlsWriteClose,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::HostName => "std.net.hostName",
            Self::IpParse => "std.net.IpAddress.parse",
            Self::SocketAddress => "std.net.socketAddress",
            Self::LimitsCreate => "std.net.NetLimits.create",
            Self::LimitsDefaults => "std.net.NetLimits.defaults",
            Self::Options => "std.net.options",
            Self::Resolve => "std.net.resolve",
            Self::Connect => "std.net.connect",
            Self::Listen => "std.net.listen",
            Self::ListenerAccept => "std.net.TcpListener.accept",
            Self::ListenerLocal => "std.net.TcpListener.localAddress",
            Self::ListenerClose => "std.net.TcpListener.close",
            Self::TcpSplit => "std.net.TcpStream.split",
            Self::TcpLocal => "std.net.TcpStream.localAddress",
            Self::TcpPeer => "std.net.TcpStream.peerAddress",
            Self::TcpShutdown => "std.net.TcpStream.shutdown",
            Self::TcpClose => "std.net.TcpStream.close",
            Self::TcpRead => "std.net.TcpReadHalf.read",
            Self::TcpReadClose => "std.net.TcpReadHalf.close",
            Self::TcpWrite => "std.net.TcpWriteHalf.write",
            Self::TcpFlush => "std.net.TcpWriteHalf.flush",
            Self::TcpWriteShutdown => "std.net.TcpWriteHalf.shutdown",
            Self::TcpWriteClose => "std.net.TcpWriteHalf.close",
            Self::Bind => "std.net.bind",
            Self::UdpSend => "std.net.UdpSocket.sendTo",
            Self::UdpReceive => "std.net.UdpSocket.receiveFrom",
            Self::UdpLocal => "std.net.UdpSocket.localAddress",
            Self::UdpClose => "std.net.UdpSocket.close",
            Self::DatagramBytes => "std.net.Datagram.bytes",
            Self::DatagramSource => "std.net.Datagram.source",
            Self::TlsConfig => "std.net.tlsConfig",
            Self::TlsConnect => "std.net.TlsStream.connect",
            Self::TlsSplit => "std.net.TlsStream.split",
            Self::TlsClose => "std.net.TlsStream.close",
            Self::TlsRead => "std.net.TlsReadHalf.read",
            Self::TlsReadClose => "std.net.TlsReadHalf.close",
            Self::TlsWrite => "std.net.TlsWriteHalf.write",
            Self::TlsFlush => "std.net.TlsWriteHalf.flush",
            Self::TlsShutdown => "std.net.TlsWriteHalf.shutdown",
            Self::TlsWriteClose => "std.net.TlsWriteHalf.close",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.name() == name)
    }

    pub fn module_function(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.name().strip_prefix("std.net.") == Some(name))
    }

    pub fn associated(owner: &str, name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|operation| {
            operation.receiver().is_none()
                && operation
                    .name()
                    .strip_prefix("std.net.")
                    .and_then(|name| name.split_once('.'))
                    == Some((owner, name))
        })
    }

    pub fn member(kind: NetworkType, name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|operation| {
            operation.receiver() == Some(kind)
                && operation
                    .name()
                    .rsplit_once('.')
                    .is_some_and(|(_, leaf)| leaf == name)
        })
    }

    pub const fn receiver(self) -> Option<NetworkType> {
        Some(match self {
            Self::ListenerAccept | Self::ListenerLocal => NetworkType::TcpListener,
            Self::TcpLocal | Self::TcpPeer | Self::TcpShutdown => NetworkType::TcpStream,
            Self::TcpRead => NetworkType::TcpReadHalf,
            Self::TcpWrite | Self::TcpFlush | Self::TcpWriteShutdown => NetworkType::TcpWriteHalf,
            Self::UdpSend | Self::UdpReceive | Self::UdpLocal => NetworkType::UdpSocket,
            Self::DatagramBytes | Self::DatagramSource => NetworkType::Datagram,
            Self::TlsRead => NetworkType::TlsReadHalf,
            Self::TlsWrite | Self::TlsFlush | Self::TlsShutdown => NetworkType::TlsWriteHalf,
            _ => return None,
        })
    }

    pub const fn suspends(self) -> bool {
        matches!(
            self,
            Self::Resolve
                | Self::Connect
                | Self::ListenerAccept
                | Self::TcpShutdown
                | Self::TcpRead
                | Self::TcpWrite
                | Self::TcpFlush
                | Self::TcpWriteShutdown
                | Self::UdpSend
                | Self::UdpReceive
                | Self::TlsConnect
                | Self::TlsRead
                | Self::TlsWrite
                | Self::TlsFlush
                | Self::TlsShutdown
        )
    }

    pub const fn selectable(self) -> bool {
        matches!(
            self,
            Self::ListenerAccept | Self::TcpRead | Self::UdpReceive
        )
    }

    /// Value-only setup may run while a source adapter is still tentative.
    /// Transport queries, I/O and lifecycle operations require commitment.
    pub const fn reversible_setup(self) -> bool {
        matches!(
            self,
            Self::HostName
                | Self::IpParse
                | Self::SocketAddress
                | Self::LimitsCreate
                | Self::LimitsDefaults
                | Self::Options
                | Self::TlsConfig
        )
    }
}

impl NetworkType {
    pub const ALL: [Self; 12] = [
        Self::NetLimits,
        Self::NetOptions,
        Self::TcpListener,
        Self::TcpStream,
        Self::TcpReadHalf,
        Self::TcpWriteHalf,
        Self::UdpSocket,
        Self::Datagram,
        Self::TlsConfig,
        Self::TlsStream,
        Self::TlsReadHalf,
        Self::TlsWriteHalf,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::NetLimits => "NetLimits",
            Self::NetOptions => "NetOptions",
            Self::TcpListener => "TcpListener",
            Self::TcpStream => "TcpStream",
            Self::TcpReadHalf => "TcpReadHalf",
            Self::TcpWriteHalf => "TcpWriteHalf",
            Self::UdpSocket => "UdpSocket",
            Self::Datagram => "Datagram",
            Self::TlsConfig => "TlsConfig",
            Self::TlsStream => "TlsStream",
            Self::TlsReadHalf => "TlsReadHalf",
            Self::TlsWriteHalf => "TlsWriteHalf",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    pub const fn owns_transport(self) -> bool {
        matches!(
            self,
            Self::TcpListener
                | Self::TcpStream
                | Self::TcpReadHalf
                | Self::TcpWriteHalf
                | Self::UdpSocket
                | Self::TlsStream
                | Self::TlsReadHalf
                | Self::TlsWriteHalf
        )
    }
}
