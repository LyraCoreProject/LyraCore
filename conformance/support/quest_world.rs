// See conformance/README.md#quest-data for fixture provenance and scope.
pub const STATEMENTS: &[&str] = &[
    "INSERT INTO game_quest_flags (quest_entry,flags) VALUES (2158,8)",
    "INSERT INTO game_quest_flags (quest_entry,flags) VALUES (433,8)",
    "INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,\
    prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,\
    rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,\
    reward_money_max_level,quest_type) VALUES (7,1,2,'Fixture quest 7',25,170,783,0,0,9,72,100,0,0,\
    0,0,false,0,0,90,0)",
    "INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) \
    VALUES (7,7,0,0,6,10)",
    "INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,\
    prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,\
    rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,\
    reward_money_max_level,quest_type) VALUES (15,1,3,'Fixture quest 15',40,250,7,0,0,9,72,100,0,\
    0,0,0,false,21,0,150,0)",
    "INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) \
    VALUES (15,15,0,0,257,10)",
    "INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,\
    prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,\
    rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,\
    reward_money_max_level,quest_type) VALUES (433,6,11,'Fixture quest 433',300,660,0,0,0,1,47,75,54,\
    75,0,0,false,0,0,390,0)",
    "INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) \
    VALUES (433,433,0,0,1117,10)",
    "INSERT INTO game_quest_text (quest_entry,details,objectives,offer_reward_text,request_items_text) \
    VALUES (433,'Fixture details.','Fixture objectives.','Fixture reward.','Fixture request.')",
    "INSERT INTO game_creature_quest (id,creature_entry,quest_entry,role) VALUES (1,1977,433,0),(2,1977,\
    433,1)",
    "INSERT INTO game_creature_template (entry,name,subname,display_id,level,health,faction_template,\
    npc_flags,unit_flags,creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,\
    money_min,money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,\
    pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class)VALUES (1977,'Fixture giver',\
    '',1570,50,1,55,2,0,7,0,0,0,1,1500,0,0,50,1,0,0,0,0,0,0,0,0)",
    "INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,reward_money_max_level,quest_type) VALUES (2158,1,5,'Fixture quest 2158',0,110,0,0,0,12,72,25,0,0,0,0,false,0,0,60,0)",
    "INSERT INTO game_quest_text (quest_entry,details,objectives,offer_reward_text,request_items_text) VALUES (2158,'Fixture details 2158.','Fixture objectives 2158.','Fixture reward 2158.','Fixture request 2158.')",
    "INSERT INTO game_quest_text (quest_entry,details,objectives,offer_reward_text,request_items_text) VALUES (7,'Fixture details 7.','Fixture objectives 7.','Fixture reward 7.','Fixture request 7.')",
    "INSERT INTO game_quest_reward_choice (id,quest_entry,choice_index,item_entry,count) VALUES (1,2158,0,4656,5),(2,2158,1,159,5)",
    "INSERT INTO game_creature_quest (id,creature_entry,quest_entry,role) VALUES (3,197,7,1),(4,295,2158,1),(5,6774,2158,0)",
];
