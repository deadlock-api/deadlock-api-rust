use haste::entities::fkey_from_path;
use haste::fxhash;

pub(super) const CONTROLLER_HASH: u64 = fxhash::hash_bytes(b"m_hController");
pub(super) const PAWN_HASH: u64 = fxhash::hash_bytes(b"m_hPawn");
pub(super) const STEAM_ID_HASH: u64 = fxhash::hash_bytes(b"m_steamID");
pub(super) const STEAM_NAME_HASH: u64 = fxhash::hash_bytes(b"m_iszPlayerName");
pub(super) const HERO_BUILD_ID_HASH: u64 = fxhash::hash_bytes(b"m_unHeroBuildID");
pub(super) const LEVEL_HASH: u64 = fxhash::hash_bytes(b"m_nLevel");
pub(super) const TEAM_HASH: u64 = fxhash::hash_bytes(b"m_iTeamNum");
pub(super) const HEALTH_HASH: u64 = fxhash::hash_bytes(b"m_iHealth");
pub(super) const MAX_HEALTH_HASH: u64 = fxhash::hash_bytes(b"m_iMaxHealth");
pub(super) const HERO_ID_HASH: u64 = fxhash::hash_bytes(b"m_nHeroID");
pub(super) const PLAYER_SLOT_HASH: u64 = fxhash::hash_bytes(b"m_unLobbyPlayerSlot");
pub(super) const RANK_HASH: u64 = fxhash::hash_bytes(b"m_nCurrentRank");
pub(super) const ASSIGNED_LANE_HASH: u64 = fxhash::hash_bytes(b"m_nAssignedLane");
pub(super) const ORIGINAL_ASSIGNED_LANE_HASH: u64 =
    fxhash::hash_bytes(b"m_nOriginalLaneAssignment");
pub(super) const NET_WORTH_HASH: u64 = fxhash::hash_bytes(b"m_iGoldNetWorth");
pub(super) const HEALTH_REGEN_HASH: u64 = fxhash::hash_bytes(b"m_flHealthRegen");
pub(super) const ULTIMATE_TRAINED_HASH: u64 = fxhash::hash_bytes(b"m_bUltimateTrained");
pub(super) const KILLS_HASH: u64 = fxhash::hash_bytes(b"m_iPlayerKills");
pub(super) const ASSISTS_HASH: u64 = fxhash::hash_bytes(b"m_iPlayerAssists");
pub(super) const DEATHS_HASH: u64 = fxhash::hash_bytes(b"m_iDeaths");
pub(super) const DENIES_HASH: u64 = fxhash::hash_bytes(b"m_iDenies");
pub(super) const LAST_HITS_HASH: u64 = fxhash::hash_bytes(b"m_iLastHits");
pub(super) const HERO_HEALING_HASH: u64 = fxhash::hash_bytes(b"m_iHeroHealing");
pub(super) const SELF_HEALING_HASH: u64 = fxhash::hash_bytes(b"m_iSelfHealing");
pub(super) const HERO_DAMAGE_HASH: u64 = fxhash::hash_bytes(b"m_iHeroDamage");
pub(super) const OBJECTIVE_DAMAGE_HASH: u64 = fxhash::hash_bytes(b"m_iObjectiveDamage");
pub(super) const ULTIMATE_COOLDOWN_END_HASH: u64 = fxhash::hash_bytes(b"m_flUltimateCooldownEnd");
pub(super) const UPGRADES_HASH: u64 = fxhash::hash_bytes(b"m_vecUpgrades");
pub(super) const CREATE_TIME_HASH: u64 = fxhash::hash_bytes(b"m_flCreateTime");
pub(super) const LANE_HASH: u64 = fxhash::hash_bytes(b"m_iLane");
pub(super) const SHIELD_ACTIVE_HASH: u64 = fxhash::hash_bytes(b"m_bShieldActive");
pub(super) const ACTIVE_HASH: u64 = fxhash::hash_bytes(b"m_bActive");
pub(super) const CX: u64 = fkey_from_path(&["CBodyComponent", "m_cellX"]);
pub(super) const CY: u64 = fkey_from_path(&["CBodyComponent", "m_cellY"]);
pub(super) const CZ: u64 = fkey_from_path(&["CBodyComponent", "m_cellZ"]);
pub(super) const VX: u64 = fkey_from_path(&["CBodyComponent", "m_vecX"]);
pub(super) const VY: u64 = fkey_from_path(&["CBodyComponent", "m_vecY"]);
pub(super) const VZ: u64 = fkey_from_path(&["CBodyComponent", "m_vecZ"]);
pub(super) const START_TIME_HASH: u64 = fkey_from_path(&["m_pGameRules", "m_flGameStartTime"]);
pub(super) const PAUSED_HASH: u64 = fkey_from_path(&["m_pGameRules", "m_bGamePaused"]);
pub(super) const PAUSE_START_TICK_HASH: u64 =
    fkey_from_path(&["m_pGameRules", "m_nPauseStartTick"]);
pub(super) const PAUSED_TICKS_HASH: u64 = fkey_from_path(&["m_pGameRules", "m_nTotalPausedTicks"]);
