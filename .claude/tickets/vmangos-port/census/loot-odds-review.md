# Milestone 2 loot odds verification

Odds are calculated from the emitted Module rows using independent group-zero rolls and the Module's cumulative basis-point bands. Before uses the original Milestone 2 importer and its retained loot SQL; after uses the repaired production plans. Percentages mean at least one drop, except expected-count rows. These are distribution calculations with uniform basis-point rolls, not random simulations. Total item counts include quest-only drops and assume the Character can take them. Item templates and random properties must be available.

| Loot entry | Item | Before | After |
| --- | --- | ---: | ---: |
| 13945 | Fel Creep 11514 | 50.0000% | 50.0000% |
| 13945 | Golden Sansam 13464 | 50.0000% | 100.0000% |
| 13946 | Fel Creep 11514 | 50.0000% | 50.0000% |
| 13946 | Dreamfoil 13463 | 50.0000% | 100.0000% |
| 13947 | Fel Creep 11514 | 50.0000% | 50.0000% |
| 13947 | Mountain Silversage 13465 | 50.0000% | 100.0000% |
| 17203 | Fel Creep 11514 | 50.0000% | 50.0000% |
| 17203 | Dreamfoil 13463 | 50.0000% | 100.0000% |
| 17204 | Fel Creep 11514 | 50.0000% | 50.0000% |
| 17204 | Mountain Silversage 13465 | 50.0000% | 100.0000% |
| 6151 | Sungrass 8838 | 100.0000% | 100.0000% |
| 6151 | Fel Creep 11514 | 0.0000% | 50.0000% |
| 13965 | Sungrass 8838 | 100.0000% | 100.0000% |
| 13965 | Fel Creep 11514 | 0.0000% | 50.0000% |
| 17201 | Sungrass 8838 | 100.0000% | 100.0000% |
| 17201 | Fel Creep 11514 | 0.0000% | 50.0000% |
| 17200 | Wildvine 8153 | 9.6000% | 9.6000% |
| 17200 | Purple Lotus 8831 | 90.4000% | 100.0000% |
| 6142 | Wildvine 8153 | 9.6000% | 9.6000% |
| 6142 | Purple Lotus 8831 | 90.4000% | 100.0000% |

The pinned Sungrass reference contains Fel Creep 11514 with runtime condition 80, not Swiftthistle 2452. The Module still lacks that runtime loot condition. Wildvine without Purple Lotus falls from 9.6% to 0%.

| Boss or chest | Example item | Before chance | After chance | Expected total item count before → after |
| --- | --- | ---: | ---: | ---: |
| Ragnaros 11502 | Legplates of Wrath 16962 | 23.4375% | 41.3818% | 11.8800 → 13.8800 |
| Nefarian 11583 | Breastplate of Wrath 16966 | 11.1100% | 21.0035% | 9.0086 → 10.0120 |
| Colossus of Zora 15740 | Hand of Edward the Odd 2243 | 33.3300% | 86.8379% | 4.9998 → 15.0000 |
| Ras Frostwhisper 10508 | Shadowy Mail Greaves 18694 | 8.3300% | 15.9844% | 3.7996 → 4.8000 |
| Lokholar 13256 | Frozen Steel Vambraces 19112 | 19.7400% | 35.5833% | 2.1040 → 3.1050 |
| Ivus 13419 | Frozen Steel Vambraces 19112 | 19.7400% | 35.5833% | 2.4323 → 3.4328 |
| Dathrohan/Balnazzar 10813 | Wyrmtongue Shoulders 13358 | 10.0000% | 19.0000% | 2.9337 → 3.9337 |
| Four Horsemen Chest 18036 | Desecrated Robe 22351 | 33.3300% | 55.5644% | 2.2999 → 3.3000 |
| Knot Thimblejack's Cache 16591 | Thick Leather 4304 | 7.6900% | 14.8071% | 9.6205 → 17.5476 |
| Magenta Cap Clusters 5212 | Magenta Fungus Cap 8047 | 100.0000% | 100.0000% | 2.0000 → 3.0000 |

Before, chest 2283 group 1: 631 members, 10096 bp, 6 unreachable members. Nonzero chances span 0.1600% to 0.1600%.

After, chest 2283 group 1: 631 members, 10000 bp, 0 unreachable members. Nonzero chances span 0.1500% to 0.1600%.
