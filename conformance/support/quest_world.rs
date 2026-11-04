// See conformance/README.md#quest-data for fixture provenance and scope.
pub const STATEMENTS: &[&str] = &[
    "INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,\
    prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,\
    rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,\
    reward_money_max_level,quest_type) VALUES (7,1,2,'Kobold Camp Cleanup',25,170,783,0,0,9,72,100,0,0,\
    0,0,false,0,0,90,0)",
    "INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) \
    VALUES (7,7,0,0,6,10)",
    "INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,\
    prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,\
    rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,\
    reward_money_max_level,quest_type) VALUES (15,1,3,'Investigate Echo Ridge',40,250,7,0,0,9,72,100,0,\
    0,0,0,false,21,0,150,0)",
    "INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) \
    VALUES (15,15,0,0,257,10)",
    "INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,\
    prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,\
    rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,\
    reward_money_max_level,quest_type) VALUES (433,6,11,'The Public Servant',300,660,0,0,0,1,47,75,54,\
    75,0,0,false,0,0,390,0)",
    "INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) \
    VALUES (433,433,0,0,1117,10)",
    "INSERT INTO game_quest_text (quest_entry,details,objectives,offer_reward_text,request_items_text) \
    VALUES (433,'As a member of the Senate and the Explorers'' League, I''ve taken it upon myself to \
    take care of this part of the trogg infestation that has gripped our lands.$b$bThey''ve certainly \
    made a mess of Gol''Bolar quarry, and for no reason. As we dug deep into the earth, they poured out, \
    destroying our equipment and driving the miners out. There''s not much for us to do but to \
    exterminate the lot of them, rebuild, then get back to work.$b$bIf you help me with the troggs, \
    I''ll gladly recompense you for your time.','Kill 10 Rockjaw Bonesnappers for Senator Mehr \
    Stonehallow at the Gol''Bolar quarry.','It is a start. Hopefully we can take advantage of it, and \
    move some Mountaineers in to secure the area. Then we can begin to restore our infrastructure and \
    get the miners back to work.$b$bThank you for your assistance, $n.','If we can remove the troggs \
    from Gol''Bolar quarry, it will be the first step of many that we will have to undertake to rid \
    ourselves of this menace.')",
    "INSERT INTO game_creature_quest (id,creature_entry,quest_entry,role) VALUES (1,1977,433,0),(2,1977,\
    433,1)",
    "INSERT INTO game_creature_template (entry,name,subname,display_id,level,health,faction_template,\
    npc_flags,unit_flags,creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,\
    money_min,money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,\
    pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class)VALUES (1977,'Senator Mehr Stonehallow',\
    '',1570,50,1,55,2,0,7,0,0,0,1,1500,0,0,50,1,0,0,0,0,0,0,0,0)",
];
