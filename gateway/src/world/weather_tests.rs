//! Zone weather at world entry and on a live World Session.

use super::*;

/// Drive one login to completion and hand back the world-entry weather packet.
fn world_entry_weather(store: std::sync::Arc<WorldFake>) -> SMSG_WEATHER {
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let mut weather = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_WEATHER(m) = message {
            weather = Some(*m);
        }
    }
    drop(client);
    server.join().unwrap();
    weather.expect("every world entry sends the zone's weather")
}

/// Story 10: a zone the Module has no weather row for is fine weather. World entry still sends the
/// packet — a client told nothing keeps rendering whatever sky it arrived with.
#[test]
fn world_entry_into_a_zone_with_no_weather_row_sends_fine_weather() {
    let weather = world_entry_weather(std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            ..base
        }
    }));
    assert_eq!(weather.weather_type, WeatherType::Fine);
    assert_eq!(weather.grade, 0.0);
    assert_eq!(weather.sound_id, 0);
    assert_eq!(weather.change, WeatherChangeType::Instant);
}

/// A Store that cannot answer the weather question is a degraded sky, never a failed login: the
/// session completes world entry and the player lands under clear skies.
#[test]
fn a_weather_read_failure_still_completes_world_entry() {
    let weather = world_entry_weather(std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            weather: WeatherState {
                weather_error: Some("shard cache unavailable".into()),
                ..base.weather
            },
            ..base
        }
    }));
    assert_eq!(weather.weather_type, WeatherType::Fine);
    assert_eq!(weather.grade, 0.0);
}

/// Drive one World Session through the handshake and world entry, and hand back the client end
/// parked on the first packet that comes AFTER world entry.
fn world_session_in_world(
    store: std::sync::Arc<WorldFake>,
    character_guid: u64,
) -> (UnixStream, DecrypterHalf, std::thread::JoinHandle<()>) {
    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(character_guid),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    (client, c_dec, server)
}

/// One `game_zone_weather` row, as the Module writes it and the relay reads it.
fn zone_weather_row(
    zone_id: u32,
    weather_type: u8,
    intensity: f32,
) -> crate::stdb::bindings::ZoneWeather {
    crate::stdb::bindings::ZoneWeather {
        zone_id,
        weather_type,
        intensity,
        changed_at_micros: 0,
    }
}

/// Stories 5 and 6 end to end, over the real cipher: two World Sessions on real socket pairs share
/// one shard's relay, one standing in Elwynn Forest and one in Westfall. Forced Elwynn rain must
/// decode as valid rain on the Elwynn client and must not reach the Westfall client at all.
///
/// Both characters stand on the SAME position, so nothing but the zone can separate them — the
/// spatial index has no say in weather.
///
/// Westfall's own snow is a sentinel rather than a bare timeout. A session's writer is FIFO, so an
/// Elwynn row that leaked into Westfall's routing would arrive on that socket BEFORE the snow; the
/// assertion therefore fails deterministically if the zone filter goes away, instead of resting on
/// a socket that happened to stay quiet. The bounded silence afterwards closes the mirror case:
/// Westfall's snow must not reach Elwynn either.
#[test]
fn forced_elwynn_rain_reaches_only_the_client_standing_in_elwynn() {
    const ELWYNN: u32 = 12;
    const WESTFALL: u32 = 40;
    let view = std::sync::Arc::new(crate::stdb::world_view::WorldView::new(true));

    let mut westfall_entity = warrior_entity();
    westfall_entity.guid = 2;
    westfall_entity.zone_id = WESTFALL;
    let session_store = |entity: codec::EntityView| {
        std::sync::Arc::new({
            let base = tester_store(7);
            WorldFake {
                session: SessionState {
                    login_entity: Some(entity),
                    relay_view: Some(view.clone()),
                    ..base.session
                },
                ..base
            }
        })
    };

    let (mut elwynn_client, mut elwynn_dec, elwynn_server) =
        world_session_in_world(session_store(warrior_entity()), 1);
    let (mut westfall_client, mut westfall_dec, westfall_server) =
        world_session_in_world(session_store(westfall_entity), 2);

    // Elwynn is forced to heavy rain; Westfall drifts into light snow of its own.
    crate::stdb::world_view::relay_zone_weather(&view, 0, &zone_weather_row(ELWYNN, 1, 0.75));
    crate::stdb::world_view::relay_zone_weather(&view, 0, &zone_weather_row(WESTFALL, 2, 0.30));

    match ServerOpcodeMessage::read_encrypted(&mut elwynn_client, &mut elwynn_dec).unwrap() {
        ServerOpcodeMessage::SMSG_WEATHER(m) => {
            assert_eq!(m.weather_type, WeatherType::Rain);
            assert_eq!(m.grade, 0.75);
            assert_eq!(m.sound_id, 8535, "0.75 is the heavy rain band");
            assert_eq!(m.change, WeatherChangeType::Smooth);
        }
        other => panic!("the Elwynn client must receive Elwynn's rain, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut westfall_client, &mut westfall_dec).unwrap() {
        ServerOpcodeMessage::SMSG_WEATHER(m) => {
            assert_eq!(
                m.weather_type,
                WeatherType::Snow,
                "the first weather the Westfall client sees must be Westfall's own — rain here \
                 means Elwynn's row leaked across the zone boundary"
            );
            assert_eq!(m.grade, 0.30);
            assert_eq!(m.sound_id, 8536, "0.30 is the light snow band");
        }
        other => panic!("the Westfall client must receive Westfall's snow, got {other}"),
    }

    // Neither zone's sky is echoed to the other. A short deadline is enough: both sockets have
    // already delivered a packet enqueued after the one being tested for.
    let quiet = std::time::Duration::from_millis(250);
    for (name, client, dec) in [
        ("Elwynn", &mut elwynn_client, &mut elwynn_dec),
        ("Westfall", &mut westfall_client, &mut westfall_dec),
    ] {
        client.set_read_timeout(Some(quiet)).unwrap();
        assert!(
            ServerOpcodeMessage::read_encrypted(client, dec).is_err(),
            "the {name} client must receive exactly one weather packet, its own zone's"
        );
    }

    drop(elwynn_client);
    drop(westfall_client);
    elwynn_server.join().unwrap();
    westfall_server.join().unwrap();
}
