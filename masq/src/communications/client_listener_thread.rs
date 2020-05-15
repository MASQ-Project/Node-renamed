use websocket::sync::Client;
use std::net::TcpStream;
use std::sync::mpsc::Sender;
use masq_lib::ui_gateway::MessageBody;
use websocket::receiver::Reader;
use std::thread;
use masq_lib::ui_traffic_converter::UiTrafficConverter;
use websocket::ws::receiver::Receiver;
use websocket::OwnedMessage;

#[derive (Clone, Copy, PartialEq, Debug)]
enum ClientListenerError {
    Closed,
    Broken,
    UnexpectedPacket,
}

impl ClientListenerError {
    fn is_fatal (&self) -> bool {
        match self {
            &ClientListenerError::Closed => true,
            &ClientListenerError::Broken => true,
            &ClientListenerError::UnexpectedPacket => false,
        }
    }
}

struct ClientListenerThread {
    listener_half: Reader<TcpStream>,
    message_body_tx: Sender<Result<MessageBody, ClientListenerError>>,
}

impl ClientListenerThread {
    pub fn new(listener_half: Reader<TcpStream>, message_body_tx: Sender<Result<MessageBody, ClientListenerError>>) -> Self {
        Self {
            listener_half,
            message_body_tx,
        }
    }

    pub fn start(mut self) {
        thread::spawn (move || {
            loop {
                match self.listener_half.receiver.recv_message(&mut self.listener_half.stream) {
                    Ok(OwnedMessage::Text (string)) => match UiTrafficConverter::new_unmarshal (&string) {
                        Ok(body) => match self.message_body_tx.send (Ok (body)) {
                            Ok (_) => (),
                            Err (_) => break,
                        },
                        Err (_) => match self.message_body_tx.send (Err (ClientListenerError::UnexpectedPacket)) {
                            Ok(_) => (),
                            Err (_) => break,
                        },
                    },
                    Ok(OwnedMessage::Close (_)) => {
                        let _ = self.message_body_tx.send (Err (ClientListenerError::Closed));
                        break;
                    },
                    Ok(_) => match self.message_body_tx.send (Err (ClientListenerError::UnexpectedPacket)) {
                        Ok(_) => (),
                        Err (_) => break,
                    },
                    Err(_) => {
                        let _ = self.message_body_tx.send (Err (ClientListenerError::Broken));
                        break;
                    },
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use masq_lib::messages::{UiShutdownResponse, UiShutdownRequest, NODE_UI_PROTOCOL};
    use masq_lib::messages::ToMessageBody;
    use crate::test_utils::mock_websockets_server::MockWebSocketsServer;
    use masq_lib::utils::{find_free_port, localhost};
    use websocket::ClientBuilder;
    use websocket::ws::sender::Sender;

    fn make_client (port: u16) -> Client<TcpStream> {
        let builder =
            ClientBuilder::new(format!("ws://{}:{}", localhost(), port).as_str()).expect("Bad URL");
        builder.add_protocol(NODE_UI_PROTOCOL).connect_insecure().unwrap()
    }

    #[test]
    fn listens_and_passes_data_through () {
        let expected_message = UiShutdownResponse{};
        let port = find_free_port();
        let server = MockWebSocketsServer::new(port)
            .queue_response(expected_message.clone().tmb(1));
        let stop_handle = server.start();
        let client = make_client(port);
        let (listener_half, mut talker_half) = client.split().unwrap();
        let (message_body_tx, message_body_rx) = mpsc::channel();
        let subject = ClientListenerThread::new(listener_half, message_body_tx);
        subject.start();
        let message = OwnedMessage::Text(UiTrafficConverter::new_marshal(UiShutdownRequest{}.tmb(1)));

        talker_half.sender.send_message(&mut talker_half.stream, &message).unwrap();

        let message_body = message_body_rx.recv().unwrap().unwrap();
        assert_eq! (message_body, expected_message.tmb(1));
        let _ = stop_handle.stop();
    }

    #[test]
    fn processes_incoming_close_correctly () {
        let port = find_free_port();
        let server = MockWebSocketsServer::new(port)
            .queue_string ("close")
            .queue_string("disconnect");
        let stop_handle = server.start();
        let client = make_client(port);
        let (listener_half, mut talker_half) = client.split().unwrap();
        let (message_body_tx, message_body_rx) = mpsc::channel();
        let subject = ClientListenerThread::new(listener_half, message_body_tx);
        subject.start();
        let message = OwnedMessage::Text(UiTrafficConverter::new_marshal(UiShutdownRequest{}.tmb(1)));

        talker_half.sender.send_message(&mut talker_half.stream, &message).unwrap();

        let error = message_body_rx.recv().unwrap().err().unwrap();
        assert_eq! (error, ClientListenerError::Closed);
        let _ = stop_handle.stop();
    }

    #[test]
    fn processes_broken_connection_correctly () {
        let port = find_free_port();
        let server = MockWebSocketsServer::new(port)
            .queue_string("disconnect");
        let stop_handle = server.start();
        let client = make_client(port);
        let (listener_half, mut talker_half) = client.split().unwrap();
        let (message_body_tx, message_body_rx) = mpsc::channel();
        let subject = ClientListenerThread::new(listener_half, message_body_tx);
        subject.start();
        let message = OwnedMessage::Text(UiTrafficConverter::new_marshal(UiShutdownRequest{}.tmb(1)));

        talker_half.sender.send_message(&mut talker_half.stream, &message).unwrap();

        let error = message_body_rx.recv().unwrap().err().unwrap();
        assert_eq! (error, ClientListenerError::Broken);
        let _ = stop_handle.stop();
    }

    #[test]
    fn processes_bad_owned_message_correctly () {
        let port = find_free_port();
        let server = MockWebSocketsServer::new(port)
            .queue_owned_message(OwnedMessage::Binary (vec![]));
        let stop_handle = server.start();
        let client = make_client(port);
        let (listener_half, mut talker_half) = client.split().unwrap();
        let (message_body_tx, message_body_rx) = mpsc::channel();
        let subject = ClientListenerThread::new(listener_half, message_body_tx);
        subject.start();
        let message = OwnedMessage::Text(UiTrafficConverter::new_marshal(UiShutdownRequest{}.tmb(1)));

        talker_half.sender.send_message(&mut talker_half.stream, &message).unwrap();

        let error = message_body_rx.recv().unwrap().err().unwrap();
        assert_eq! (error, ClientListenerError::UnexpectedPacket);
        let _ = stop_handle.stop();
    }

    #[test]
    fn processes_bad_packet_correctly () {
        let port = find_free_port();
        let server = MockWebSocketsServer::new(port)
            .queue_string("booga");
        let stop_handle = server.start();
        let client = make_client(port);
        let (listener_half, mut talker_half) = client.split().unwrap();
        let (message_body_tx, message_body_rx) = mpsc::channel();
        let subject = ClientListenerThread::new(listener_half, message_body_tx);
        subject.start();
        let message = OwnedMessage::Text(UiTrafficConverter::new_marshal(UiShutdownRequest{}.tmb(1)));

        talker_half.sender.send_message(&mut talker_half.stream, &message).unwrap();

        let error = message_body_rx.recv().unwrap().err().unwrap();
        assert_eq! (error, ClientListenerError::UnexpectedPacket);
        let _ = stop_handle.stop();
    }

    #[test]
    fn client_listener_errors_know_their_own_fatality () {
        assert_eq! (ClientListenerError::Closed.is_fatal(), true);
        assert_eq! (ClientListenerError::Broken.is_fatal(), true);
        assert_eq! (ClientListenerError::UnexpectedPacket.is_fatal(), false);
    }
}