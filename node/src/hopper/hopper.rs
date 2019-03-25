// Copyright (c) 2017-2019, Substratum LLC (https://substratum.net) and/or its affiliates. All rights reserved.
use super::consuming_service::ConsumingService;
use super::routing_service::RoutingService;
use crate::sub_lib::cryptde::CryptDE;
use crate::sub_lib::dispatcher::InboundClientData;
use crate::sub_lib::hopper::HopperConfig;
use crate::sub_lib::hopper::HopperSubs;
use crate::sub_lib::hopper::IncipientCoresPackage;
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::utils::NODE_MAILBOX_CAPACITY;
use actix::Actor;
use actix::Addr;
use actix::Context;
use actix::Handler;

pub struct Hopper {
    cryptde: &'static dyn CryptDE,
    is_bootstrap_node: bool,
    consuming_service: Option<ConsumingService>,
    routing_service: Option<RoutingService>,
    per_routing_service: u64,
    per_routing_byte: u64,
}

impl Actor for Hopper {
    type Context = Context<Self>;
}

impl Handler<BindMessage> for Hopper {
    type Result = ();

    fn handle(&mut self, msg: BindMessage, ctx: &mut Self::Context) -> Self::Result {
        ctx.set_mailbox_capacity(NODE_MAILBOX_CAPACITY);
        self.consuming_service = Some(ConsumingService::new(
            self.cryptde,
            self.is_bootstrap_node,
            msg.peer_actors.dispatcher.from_dispatcher_client.clone(),
            msg.peer_actors.hopper.from_dispatcher,
        ));
        self.routing_service = Some(RoutingService::new(
            self.cryptde,
            self.is_bootstrap_node,
            msg.peer_actors.proxy_client,
            msg.peer_actors.proxy_server,
            msg.peer_actors.neighborhood,
            msg.peer_actors.dispatcher.from_dispatcher_client,
            msg.peer_actors.accountant.report_routing_service_provided,
            self.per_routing_service,
            self.per_routing_byte,
        ));
        ()
    }
}

// TODO: Make this message return a Future, so that the Proxy Server (or whatever) can tell if its
// message didn't go through.
impl Handler<IncipientCoresPackage> for Hopper {
    type Result = ();

    fn handle(&mut self, msg: IncipientCoresPackage, _ctx: &mut Self::Context) -> Self::Result {
        self.consuming_service
            .as_ref()
            .expect("Hopper unbound: no ConsumingService")
            .consume(msg);
    }
}

impl Handler<InboundClientData> for Hopper {
    type Result = ();

    fn handle(&mut self, msg: InboundClientData, _ctx: &mut Self::Context) -> Self::Result {
        self.routing_service
            .as_ref()
            .expect("Hopper unbound: no RoutingService")
            .route(msg);
    }
}

impl Hopper {
    pub fn new(config: HopperConfig) -> Hopper {
        Hopper {
            cryptde: config.cryptde,
            is_bootstrap_node: config.is_bootstrap_node,
            consuming_service: None,
            routing_service: None,
            per_routing_service: config.per_routing_service,
            per_routing_byte: config.per_routing_byte,
        }
    }

    pub fn make_subs_from(addr: &Addr<Hopper>) -> HopperSubs {
        HopperSubs {
            bind: addr.clone().recipient::<BindMessage>(),
            from_hopper_client: addr.clone().recipient::<IncipientCoresPackage>(),
            from_dispatcher: addr.clone().recipient::<InboundClientData>(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::live_cores_package::LiveCoresPackage;
    use super::*;
    use crate::sub_lib::cryptde::PlainData;
    use crate::sub_lib::cryptde::PublicKey;
    use crate::sub_lib::dispatcher::Component;
    use crate::sub_lib::hopper::{IncipientCoresPackage, MessageType};
    use crate::sub_lib::route::Route;
    use crate::sub_lib::route::RouteSegment;
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::test_utils::cryptde;
    use crate::test_utils::test_utils::route_to_proxy_client;
    use actix::Actor;
    use actix::System;
    use std::net::SocketAddr;
    use std::str::FromStr;

    #[test]
    #[should_panic(expected = "Hopper unbound: no RoutingService")]
    fn panics_if_routing_service_is_unbound() {
        let cryptde = cryptde();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let route = route_to_proxy_client(&cryptde.public_key(), cryptde);
        let serialized_payload = serde_cbor::ser::to_vec(&MessageType::DnsResolveFailed).unwrap();
        let data = cryptde
            .encode(
                &cryptde.public_key(),
                &PlainData::new(&serialized_payload[..]),
            )
            .unwrap();
        let live_package = LiveCoresPackage::new(route, data);
        let live_data = PlainData::new(&serde_cbor::ser::to_vec(&live_package).unwrap()[..]);
        let encrypted_package = cryptde
            .encode(&cryptde.public_key(), &live_data)
            .unwrap()
            .into();

        let inbound_client_data = InboundClientData {
            peer_addr,
            reception_port: None,
            last_data: false,
            is_clandestine: false,
            sequence_number: None,
            data: encrypted_package,
        };
        let system = System::new("panics_if_routing_service_is_unbound");
        let subject = Hopper::new(HopperConfig {
            cryptde,
            is_bootstrap_node: false,
            per_routing_service: 100,
            per_routing_byte: 200,
        });
        let subject_addr: Addr<Hopper> = subject.start();

        subject_addr.try_send(inbound_client_data).unwrap();

        System::current().stop_with_code(0);
        system.run();
    }

    #[test]
    #[should_panic(expected = "Hopper unbound: no ConsumingService")]
    fn panics_if_consuming_service_is_unbound() {
        let cryptde = cryptde();
        let consuming_wallet = Wallet::new("wallet");
        let next_key = PublicKey::new(&[65, 65, 65]);
        let route = Route::one_way(
            RouteSegment::new(
                vec![&cryptde.public_key(), &next_key],
                Component::Neighborhood,
            ),
            cryptde,
            Some(consuming_wallet),
        )
        .unwrap();
        let incipient_package = IncipientCoresPackage::new(
            cryptde,
            route,
            MessageType::DnsResolveFailed,
            &cryptde.public_key(),
        )
        .unwrap();
        let system = System::new("panics_if_consuming_service_is_unbound");
        let subject = Hopper::new(HopperConfig {
            cryptde,
            is_bootstrap_node: false,
            per_routing_service: 100,
            per_routing_byte: 200,
        });
        let subject_addr: Addr<Hopper> = subject.start();

        subject_addr.try_send(incipient_package).unwrap();

        System::current().stop_with_code(0);
        system.run();
    }
}
