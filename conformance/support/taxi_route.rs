// Client build 5875 route 6; see conformance/README.md#taxi-data for the Golden Trace check.
const POINTS: [(u32, [f64; 3]); 28] = [
    (35, [-8851.658203125, 498.5517578125, 111.25601959228516]),
    (
        36,
        [-8853.7666015625, 496.56976318359375, 111.51502990722656],
    ),
    (
        37,
        [-8869.408203125, 486.37249755859375, 116.43289184570312],
    ),
    (
        38,
        [-8937.974609375, 447.69573974609375, 122.10063934326172],
    ),
    (39, [-9025.54296875, 501.72021484375, 122.10063934326172]),
    (40, [-9109.89453125, 473.5466613769531, 148.98922729492188]),
    (41, [-9247.455078125, 381.1439514160156, 152.1426239013672]),
    (
        42,
        [-9328.7470703125, 433.8290100097656, 130.61520385742188],
    ),
    (43, [-9380.046875, 432.5325622558594, 78.04019165039062]),
    (44, [-9451.3798828125, 386.5201721191406, 56.9287109375]),
    (
        45,
        [-9524.6279296875, 368.06781005859375, 53.43006896972656],
    ),
    (46, [-9601.1328125, 407.5400390625, 52.476253509521484]),
    (47, [-9686.1650390625, 469.8009033203125, 40.71202850341797]),
    (
        48,
        [-9734.6220703125, 552.1638793945312, 39.323143005371094],
    ),
    (
        2918,
        [-9742.7646484375, 621.3524169921875, 37.924842834472656],
    ),
    (
        2919,
        [-9790.908203125, 641.0512084960938, 44.37460708618164],
    ),
    (2920, [-9847.7421875, 632.7854614257812, 44.14869689941406]),
    (2921, [-9941.953125, 634.8231201171875, 44.32913589477539]),
    (
        2922,
        [-10000.455078125, 691.7058715820312, 38.249725341796875],
    ),
    (
        2923,
        [-10034.64453125, 787.4933471679688, 31.297611236572266],
    ),
    (
        2924,
        [-10054.439453125, 872.945068359375, 42.68251037597656],
    ),
    (2925, [-10139.359375, 931.7116088867188, 46.93581008911133]),
    (
        2926,
        [-10255.1123046875, 986.2713623046875, 39.568214416503906],
    ),
    (
        2927,
        [-10310.583984375, 1070.4471435546875, 51.851314544677734],
    ),
    (
        2928,
        [-10364.2509765625, 1112.3980712890625, 43.31549835205078],
    ),
    (
        2929,
        [-10489.6533203125, 1100.4761962890625, 59.45343780517578],
    ),
    (
        2930,
        [-10588.3193359375, 1075.8790283203125, 47.02614212036133],
    ),
    (
        2931,
        [-10629.0654296875, 1039.445556640625, 37.9543342590332],
    ),
];
const NODES: [(u32, [f64; 3]); 2] = [
    (
        2,
        [-8840.5595703125, 489.70001220703125, 109.61000061035156],
    ),
    (
        4,
        [-10628.900390625, 1036.6800537109375, 34.060001373291016],
    ),
];
const MOUNT_DISPLAY: u32 = 6852;

// See conformance/README.md#taxi-data for the route-cell boundary choices.
const AREA_CELLS: &[(u32, &[(u16, u16)])] = &[
    (
        1519,
        &[
            (777, 497),
            (778, 497),
            (778, 498),
            (779, 498),
            (780, 498),
            (781, 498),
            (781, 497),
            (782, 497),
            (782, 496),
            (783, 496),
            (783, 497),
            (784, 497),
            (785, 497),
        ],
    ),
    (
        12,
        &[
            (785, 498),
            (786, 498),
            (786, 499),
            (787, 499),
            (788, 499),
            (788, 500),
            (789, 500),
            (790, 500),
            (790, 499),
            (791, 499),
            (791, 498),
            (792, 498),
            (793, 498),
            (793, 499),
            (794, 499),
            (794, 500),
            (795, 500),
            (796, 500),
            (797, 500),
            (798, 500),
            (799, 500),
            (799, 499),
            (800, 499),
            (801, 499),
            (801, 498),
            (802, 498),
            (802, 497),
            (803, 497),
            (803, 496),
            (803, 495),
            (804, 495),
            (804, 494),
            (804, 493),
            (804, 492),
            (805, 492),
            (806, 492),
            (807, 492),
            (807, 493),
            (808, 493),
            (809, 493),
            (810, 493),
            (810, 492),
            (811, 492),
            (811, 491),
            (812, 491),
            (812, 490),
        ],
    ),
    (
        40,
        &[
            (812, 489),
            (812, 488),
            (813, 488),
            (813, 487),
            (813, 486),
            (813, 485),
            (814, 485),
            (814, 484),
            (815, 484),
            (816, 484),
            (816, 483),
            (817, 483),
            (818, 483),
            (818, 482),
            (819, 482),
            (820, 482),
            (820, 481),
            (820, 480),
            (821, 480),
            (821, 479),
            (822, 479),
            (822, 478),
            (823, 478),
            (824, 478),
            (825, 478),
            (826, 478),
            (826, 479),
            (827, 479),
            (828, 479),
            (829, 479),
            (830, 479),
            (830, 480),
        ],
    ),
];

pub fn terrain_chunks() -> String {
    let heights = vec!["0"; 145].join(":");
    let heights = heights.as_str();
    AREA_CELLS
        .iter()
        .flat_map(|(area, cells)| {
            cells
                .iter()
                .map(move |(x, y)| format!("0,{x},{y},0,0,0,{area},{heights}"))
        })
        .collect::<Vec<_>>()
        .join(";")
}

pub fn statements(scenario: &wire_client::scenario::Scenario) -> anyhow::Result<Vec<String>> {
    use anyhow::Context;
    use wire_client::scenario::FixtureVerb;

    let flight_master = scenario.fixtures.iter().find_map(|fixture| match fixture {
        FixtureVerb::Creature { id, entry, .. } if id == "flight_master" => Some(entry),
        _ => None,
    });
    let flight_master = flight_master.context("taxi scenario flight master fixture")?;
    let mut statements = Vec::new();
    statements.push(
        "INSERT INTO game_area \
        (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) \
        VALUES (1519,0,0,1,312,10,2,'Stormwind City'),\
        (12,0,0,2,64,0,2,'Elwynn Forest'),(40,0,0,3,64,0,2,'Westfall')"
            .into(),
    );
    for (id, [x, y, z]) in NODES {
        statements.push(format!(
            "INSERT INTO game_taxi_node \
             (id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance) \
             VALUES ({id},{id},0,{x},{y},{z},'Route fixture',0,{MOUNT_DISPLAY})"
        ));
    }
    statements.push("INSERT INTO game_taxi_path (id,source_node_id,destination_node_id,fare) VALUES (6,2,4,110)".into());
    let rows = POINTS
        .iter()
        .enumerate()
        .map(|(index, (id, [x, y, z]))| format!("({id},6,{index},0,{x},{y},{z},0,0)"))
        .collect::<Vec<_>>()
        .join(",");
    statements.push(format!(
        "INSERT INTO game_taxi_path_node (id,path_id,node_index,map_id,x,y,z,flags,delay_ms) VALUES {rows}"
    ));
    statements.push(
        format!("INSERT INTO game_creature_template \
         (entry,name,subname,display_id,level,health,faction_template,npc_flags,unit_flags,\
          creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,money_min,\
          money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,\
          pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class) \
         VALUES ({flight_master},'Flight fixture','',1,1,100,35,8,0,7,0,0,0,1,2000,0,0,1,100,0,0,0,0,0,0,0,0)"),
    );
    Ok(statements)
}

#[test]
fn route_fixture_matches_every_recorded_route_point_and_mount() -> anyhow::Result<()> {
    use anyhow::Context;
    use wire_client::trace::RecordedField;

    let golden = wire_client::catalogue::get("taxi_single_hop")?;
    let fixture = statements(golden.scenario())?;
    assert!(fixture
        .iter()
        .any(|statement| statement.contains("VALUES (523,'Flight fixture'")));
    let spline = golden
        .trace()
        .packets
        .iter()
        .find(|packet| packet.checkpoint == "flight" && packet.opcode == 0x00DD)
        .context("Golden Trace passenger spline")?;
    let point = |coordinates: [f64; 3]| {
        RecordedField::Object(
            ["x", "y", "z"]
                .into_iter()
                .zip(coordinates)
                .map(|(axis, value)| (axis.into(), RecordedField::Float(value)))
                .collect(),
        )
    };
    assert_eq!(spline.fields["position"], point(NODES[0].1));
    // The wire spline starts at the passenger; its destinations omit DBC vertex zero.
    assert_eq!(
        spline.fields["points"],
        RecordedField::List(
            POINTS[1..]
                .iter()
                .map(|(_, coordinates)| point(*coordinates))
                .collect()
        )
    );
    let passenger = &spline.fields["guid"];
    let mounted = golden
        .trace()
        .packets
        .iter()
        .filter(|packet| packet.checkpoint == "flight" && packet.opcode == 0x00A9)
        .any(|packet| {
            let Some(RecordedField::List(objects)) = packet.fields.get("objects") else {
                return false;
            };
            objects.iter().any(|object| {
                let RecordedField::Object(object) = object else {
                    return false;
                };
                let Some(RecordedField::Object(values)) = object.get("values") else {
                    return false;
                };
                object.get("guid") == Some(passenger)
                    && values.get("field_133")
                        == Some(&RecordedField::Unsigned(MOUNT_DISPLAY.into()))
            })
        });
    assert!(
        mounted,
        "Golden Trace passenger must use the fixture mount display"
    );
    Ok(())
}
