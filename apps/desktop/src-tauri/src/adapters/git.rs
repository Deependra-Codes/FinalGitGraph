use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{hash_map::Entry, BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gix::bstr::ByteSlice;
use gix::diff::Rewrites;
use gix::hash::ObjectId;
use gix::object::tree::diff::ChangeDetached;
use gix::prelude::TreeDiffChangeExt;
use gix::Repository;
use gix_pack::cache::lru::MemoryCappedHashmap as PackDecodeLruCache;
use gix_pack::data::decode::entry::ResolvedBase as PackResolvedBase;
use gix_pack::data::entry::Header as PackEntryHeader;
use gix_pack::data::File as PackDataFile;
use gix_pack::index::File as PackIndexFile;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;

use crate::domain::models::{
    CommitMetadataState, FileChangeLineStatsUpdate, FileChangeStatsState, ParsedCommit,
    ParsedFileChange, ParsedRef, PendingFileChangeEnrichment,
};
use crate::support::error::{AppError, AppResult};

pub const FILE_CHANGE_COMMIT_BATCH_SIZE: usize = 1024;
const FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE: usize = 16;
const FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE: usize = 4;
const FILE_CHANGE_PARSE_PARALLEL_TARGET_CHUNKS_PER_WORKER: usize = 2;
const FILE_CHANGE_PARSE_CHANNEL_CAPACITY: usize = 8;
const FILE_CHANGE_PARSE_CHANNEL_CAPACITY_PER_WORKER: usize = 2;
const FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX: usize = 64;
const FILE_CHANGE_PARSE_WORKER_THREADS_DEFAULT_MAX: usize = 8;
const FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS_DEFAULT_FLOOR: usize = 8;
const FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS_PER_WORKER: usize = 2;
const FILE_CHANGE_PARSE_WORKER_THREADS_ENV: &str = "GITGRAPH_FILE_CHANGE_PARALLEL_THREADS";
const FILE_CHANGE_PARSE_MIN_COMMITS_ENV: &str = "GITGRAPH_FILE_CHANGE_PARALLEL_MIN_COMMITS";
const FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE_ENV: &str =
    "GITGRAPH_FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE";
const FILE_CHANGE_DRAIN_CHANNEL_CAPACITY_ENV: &str = "GITGRAPH_FILE_CHANGE_DRAIN_CHANNEL_CAPACITY";
const FILE_CHANGE_DIFF_REPO_API_FALLBACK_ENV: &str = "GITGRAPH_FILE_CHANGE_DIFF_REPO_API";
const FILE_CHANGE_SYNC_STRICT_ENV: &str = "GITGRAPH_FILE_CHANGE_SYNC_STRICT";
const FILE_CHANGE_RENAME_HOT_PATH_ENV: &str = "GITGRAPH_FILE_CHANGE_RENAME_HOT_PATH";
const FILE_CHANGE_PARENT_TREE_MEMO_ENV: &str = "GITGRAPH_FILE_CHANGE_PARENT_TREE_MEMO";
const LEVEL3_TOPO_EXTRACTION_ORDER_ENV: &str = "GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER";
const LEVEL3_PACK_INDEX_MAX_OBJECTS_ENV: &str = "GITGRAPH_LEVEL3_PACK_INDEX_MAX_OBJECTS";
const COMMIT_WALK_SORT_ENV: &str = "GITGRAPH_COMMIT_WALK_SORT";
const COMMIT_WALK_SCOPE_ENV: &str = "GITGRAPH_COMMIT_WALK_SCOPE";
const FILE_CHANGE_RENAME_LIMIT: usize = 400;
const FILE_CHANGE_RENAME_THRESHOLD_SMALL: u16 = 60;
const FILE_CHANGE_RENAME_THRESHOLD_MEDIUM: u16 = 65;
const FILE_CHANGE_RENAME_THRESHOLD_LARGE: u16 = 70;
const FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES: u64 = 64 * 1024;
const FILE_CHANGE_RENAME_LARGE_FILE_BYTES: u64 = 256 * 1024;
const FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS: usize = 512;
const FILE_CHANGE_EXACT_MATCH_ONLY_FILE_BYTES: u64 = 1_024 * 1_024;
const FILE_CHANGE_RENAME_LIMIT_LARGE_DEFAULT: usize = 200;
const FILE_CHANGE_RENAME_LIMIT_XLARGE_DEFAULT: usize = 100;
const FILE_CHANGE_RENAME_LIMIT_LARGE_ENV: &str = "GITGRAPH_FILE_CHANGE_RENAME_LIMIT_LARGE";
const FILE_CHANGE_RENAME_LIMIT_XLARGE_ENV: &str = "GITGRAPH_FILE_CHANGE_RENAME_LIMIT_XLARGE";
const FILE_CHANGE_RENAME_LIMIT_XLARGE_MIN_DELTAS: usize = 2_048;
const PATCH_FROM_DIFF_GUARDRAIL_MARKER: &str = "Patch::from_diff";
const MIN_OBJECT_ID_HEX_LEN: usize = 40;
const GIX_OBJECT_CACHE_BYTES_DEFAULT: usize = 64 * 1024 * 1024;
const GIX_OBJECT_CACHE_BYTES_MIN: usize = 16 * 1024 * 1024;
const GIX_OBJECT_CACHE_BYTES_MAX: usize = GIX_OBJECT_CACHE_BYTES_DEFAULT;
const FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE_DEFAULT: usize = 64;
const FILE_CHANGE_DRAIN_CHANNEL_CAPACITY_DEFAULT: usize = 32;
const FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE_MAX: usize = 512;
const FILE_CHANGE_DRAIN_CHANNEL_CAPACITY_MAX: usize = 256;
const FILE_CHANGE_PARENT_TREE_MEMO_MIN_COMMITS: usize = 8;
const COMMIT_GRAPH_SIGNATURE: &[u8; 4] = b"CGPH";
const COMMIT_GRAPH_CHUNK_LOOKUP_HEADER_LEN: usize = 8;
const COMMIT_GRAPH_CHUNK_LOOKUP_ENTRY_LEN: usize = 12;
const COMMIT_GRAPH_CHUNK_ID_OIDF: [u8; 4] = *b"OIDF";
const COMMIT_GRAPH_CHUNK_ID_OIDL: [u8; 4] = *b"OIDL";
const COMMIT_GRAPH_CHUNK_ID_BIDX: [u8; 4] = *b"BIDX";
const COMMIT_GRAPH_CHUNK_ID_BDAT: [u8; 4] = *b"BDAT";
const COMMIT_GRAPH_FANOUT_ENTRY_COUNT: usize = 256;
const COMMIT_GRAPH_FANOUT_CHUNK_LEN: usize = COMMIT_GRAPH_FANOUT_ENTRY_COUNT * 4;
const BLOOM_DATA_HEADER_LEN: usize = 12;
const BLOOM_MURMUR_SEED0: u32 = 0x293a_e76f;
const BLOOM_MURMUR_SEED1: u32 = 0x7e64_6e2c;
const BLOOM_HASH_VERSION_GIT_V1: u32 = 1;
const BLOOM_HASH_VERSION_GIT_V2: u32 = 2;
const LEVEL3_PACK_INDEX_MAX_OBJECTS_DEFAULT: usize = 500_000;
const PACK_DELTA_CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitWalkSortMode {
    CommitTimeNewest,
    BreadthFirst,
    CommitTimeCutoff,
}

impl CommitWalkSortMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::CommitTimeNewest => "commit_time_newest",
            Self::BreadthFirst => "breadth_first",
            Self::CommitTimeCutoff => "commit_time_cutoff",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitWalkBoundaryMode {
    NotApplicable,
    CheckpointStop,
}

impl CommitWalkBoundaryMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::CheckpointStop => "checkpoint_stop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitWalkEngine {
    Tape,
}

impl CommitWalkEngine {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tape => "tape",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CommitWalkPhaseTiming {
    decode: Duration,
    parent_resolve: Duration,
    metadata_parse: Duration,
    tip_collection: Duration,
    hide_expansion: Duration,
    graph_lookup: Duration,
    ordering: Duration,
    emit: Duration,
    pack_inflate: Duration,
    pack_base_resolve: Duration,
    pack_parse: Duration,
    tape_build: Duration,
    graph_lookup_hit_count: usize,
    graph_lookup_miss_count: usize,
    graph_assist_hit_count: usize,
    graph_assist_miss_count: usize,
    nodes_seen_count: usize,
    nodes_pruned_count: usize,
    queue_max_depth: usize,
    metadata_pending_count: usize,
    graph_missing_tip_count: usize,
    boundary_stop_count: usize,
    overlap_skip_count: usize,
    tape_node_count: usize,
    tape_parent_edges: usize,
}

impl CommitWalkPhaseTiming {
    fn merge_from(&mut self, other: Self) {
        self.decode = self.decode.saturating_add(other.decode);
        self.parent_resolve = self.parent_resolve.saturating_add(other.parent_resolve);
        self.metadata_parse = self.metadata_parse.saturating_add(other.metadata_parse);
        self.tip_collection = self.tip_collection.saturating_add(other.tip_collection);
        self.hide_expansion = self.hide_expansion.saturating_add(other.hide_expansion);
        self.graph_lookup = self.graph_lookup.saturating_add(other.graph_lookup);
        self.ordering = self.ordering.saturating_add(other.ordering);
        self.emit = self.emit.saturating_add(other.emit);
        self.pack_inflate = self.pack_inflate.saturating_add(other.pack_inflate);
        self.pack_base_resolve = self
            .pack_base_resolve
            .saturating_add(other.pack_base_resolve);
        self.pack_parse = self.pack_parse.saturating_add(other.pack_parse);
        self.tape_build = self.tape_build.saturating_add(other.tape_build);
        self.graph_lookup_hit_count = self
            .graph_lookup_hit_count
            .saturating_add(other.graph_lookup_hit_count);
        self.graph_lookup_miss_count = self
            .graph_lookup_miss_count
            .saturating_add(other.graph_lookup_miss_count);
        self.graph_assist_hit_count = self
            .graph_assist_hit_count
            .saturating_add(other.graph_assist_hit_count);
        self.graph_assist_miss_count = self
            .graph_assist_miss_count
            .saturating_add(other.graph_assist_miss_count);
        self.nodes_seen_count = self.nodes_seen_count.saturating_add(other.nodes_seen_count);
        self.nodes_pruned_count = self
            .nodes_pruned_count
            .saturating_add(other.nodes_pruned_count);
        self.queue_max_depth = self.queue_max_depth.max(other.queue_max_depth);
        self.metadata_pending_count = self
            .metadata_pending_count
            .saturating_add(other.metadata_pending_count);
        self.graph_missing_tip_count = self
            .graph_missing_tip_count
            .saturating_add(other.graph_missing_tip_count);
        self.boundary_stop_count = self
            .boundary_stop_count
            .saturating_add(other.boundary_stop_count);
        self.overlap_skip_count = self
            .overlap_skip_count
            .saturating_add(other.overlap_skip_count);
        self.tape_node_count = self.tape_node_count.saturating_add(other.tape_node_count);
        self.tape_parent_edges = self
            .tape_parent_edges
            .saturating_add(other.tape_parent_edges);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommitWalkOutcome {
    engine: CommitWalkEngine,
    boundary_mode: CommitWalkBoundaryMode,
    pruned_parent_expansions: usize,
    phase_timing: CommitWalkPhaseTiming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommitWalkPolicy {
    sort_mode: CommitWalkSortMode,
    sort_reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommitWalkContext<'a> {
    sort_mode: CommitWalkSortMode,
    since_epoch: Option<i64>,
    hide_ancestor_sha: Option<&'a str>,
    walk_scope: CommitWalkScope,
    boundary: CommitWalkBoundaryMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitWalkScope {
    Frontier,
    AllRefs,
}

impl CommitWalkScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Frontier => "frontier",
            Self::AllRefs => "all_refs",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommitWalkExecutionStats {
    pub scope: String,
    pub engine: String,
    pub sort_effective: String,
    pub boundary_mode: String,
    pub tip_count: usize,
    pub pruned_parent_expansions: usize,
    pub decode: Duration,
    pub parent_resolve: Duration,
    pub metadata_parse: Duration,
    pub tip_collection: Duration,
    pub hide_expansion: Duration,
    pub graph_lookup: Duration,
    pub ordering: Duration,
    pub emit: Duration,
    pub pack_inflate: Duration,
    pub pack_base_resolve: Duration,
    pub pack_parse: Duration,
    pub tape_build: Duration,
    pub graph_lookup_hit_count: usize,
    pub graph_lookup_miss_count: usize,
    pub graph_assist_hit_count: usize,
    pub graph_assist_miss_count: usize,
    pub nodes_seen_count: usize,
    pub nodes_pruned_count: usize,
    pub queue_max_depth: usize,
    pub metadata_pending_count: usize,
    pub graph_missing_tip_count: usize,
    pub boundary_stop_count: usize,
    pub overlap_skip_count: usize,
    pub tape_node_count: usize,
    pub tape_parent_edges: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileChangeExtractionMode {
    MetadataOnly,
    SelectiveLineStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileChangeParallelProfile {
    Interactive,
    Drain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenameDetectionPolicy {
    rename_limit: usize,
    rename_threshold: u16,
    exact_match_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParallelFileChangePlan {
    chunk_size: usize,
    channel_capacity: usize,
    wave_chunk_limit: usize,
}

#[derive(Debug, Clone)]
struct DiffLineStatTarget {
    diff_index: usize,
    path: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileDiffTelemetry {
    pub diff_build: Duration,
    pub rename_find: Duration,
    pub delta_emit: Duration,
}

impl FileDiffTelemetry {
    fn merge(&mut self, other: Self) {
        self.diff_build = self.diff_build.saturating_add(other.diff_build);
        self.rename_find = self.rename_find.saturating_add(other.rename_find);
        self.delta_emit = self.delta_emit.saturating_add(other.delta_emit);
    }

    pub fn total(&self) -> Duration {
        self.diff_build
            .saturating_add(self.rename_find)
            .saturating_add(self.delta_emit)
    }
}

#[derive(Debug)]
struct ChunkFileChanges {
    changes: Vec<ParsedFileChange>,
    telemetry: FileDiffTelemetry,
}

#[derive(Debug, Clone)]
pub struct CanonicalRepo {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Level3RuntimeOptions {
    pub topology_aware_extraction: bool,
}

#[derive(Debug, Clone)]
struct CommitGraphBloomIndex {
    layers: Vec<CommitGraphBloomLayer>,
}

#[derive(Debug, Clone)]
struct CommitGraphBloomLayer {
    data: Vec<u8>,
    oid_fan_offset: usize,
    oid_lookup_offset: usize,
    commit_count: usize,
    hash_len: usize,
    bloom_index_offset: usize,
    bloom_data_offset: usize,
    bloom_data_end: usize,
    bloom_hash_version: u32,
    bloom_num_hashes: u32,
}

#[derive(Debug, Clone, Copy)]
struct BloomFilterView<'a> {
    bytes: &'a [u8],
    hash_version: u32,
    num_hashes: u32,
}

#[derive(Debug, Default)]
struct CommitGraphBloomCache {
    by_git_dir: HashMap<PathBuf, Option<Arc<CommitGraphBloomIndex>>>,
}

#[derive(Default)]
struct CommitGraphMetadataCache {
    by_git_dir: HashMap<PathBuf, Option<Arc<gix::commitgraph::Graph>>>,
}

#[derive(Debug, Clone, Copy)]
struct PrehashedBloomProbe {
    hash0_v1: u32,
    hash1_v1: u32,
    hash0_v2: u32,
    hash1_v2: u32,
}

#[derive(Debug, Clone)]
struct PathBloomProbePlan {
    prehashed_keys: Vec<PrehashedBloomProbe>,
}

#[derive(Debug, Clone, Copy)]
struct PackObjectLocation {
    pack_slot: usize,
    pack_offset: u64,
}

struct PackBundle {
    index: PackIndexFile,
    pack: PackDataFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackFileFingerprint {
    idx_path: PathBuf,
    idx_len: u64,
    idx_modified_seconds: u64,
    pack_path: PathBuf,
    pack_len: u64,
    pack_modified_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackTraversalFingerprint {
    entries: Vec<PackFileFingerprint>,
}

type PackAwareParentIds = SmallVec<[ObjectId; 2]>;
type PackAwareTapePosition = u32;
type PackAwareTapeParentPositions = SmallVec<[PackAwareTapePosition; 2]>;
type FastObjectIdMap<T> = FxHashMap<ObjectId, T>;
type FastObjectIdSet = FxHashSet<ObjectId>;
type PackDecodeCache = PackDecodeLruCache;

#[derive(Debug, Clone)]
struct PackAwareCommitRecord {
    tree_id: Option<ObjectId>,
    parent_ids: PackAwareParentIds,
    committer_time: i64,
}

#[derive(Debug, Clone)]
struct PackAwareTapeNode {
    oid: ObjectId,
    sha: String,
    tree_id: Option<ObjectId>,
    committer_time: i64,
    parent_positions: PackAwareTapeParentPositions,
}

impl PackAwareTapeNode {
    fn placeholder(oid: ObjectId) -> Self {
        Self {
            sha: oid.to_string(),
            oid,
            tree_id: None,
            committer_time: 0,
            parent_positions: PackAwareTapeParentPositions::new(),
        }
    }

    fn to_pending_parsed_commit(&self, tape: &PackAwareTraversalTape) -> ParsedCommit {
        ParsedCommit {
            sha: self.sha.clone(),
            parents: self
                .parent_positions
                .iter()
                .filter_map(|position| tape.nodes.get(*position as usize))
                .map(|parent| parent.sha.clone())
                .collect(),
            tree_sha: self
                .tree_id
                .map(|tree_id| tree_id.to_string())
                .unwrap_or_default(),
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            committer_time: self.committer_time,
            subject: String::new(),
            metadata_state: CommitMetadataState::Pending,
        }
    }
}

#[derive(Debug, Default)]
struct PackAwareTraversalTape {
    nodes: Vec<PackAwareTapeNode>,
    positions_by_oid: FastObjectIdMap<PackAwareTapePosition>,
    parent_edge_count: usize,
}

impl PackAwareTraversalTape {
    fn ensure_position(&mut self, oid: ObjectId) -> PackAwareTapePosition {
        if let Some(position) = self.positions_by_oid.get(&oid).copied() {
            return position;
        }
        let position = PackAwareTapePosition::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        self.nodes.push(PackAwareTapeNode::placeholder(oid));
        self.positions_by_oid.insert(oid, position);
        position
    }
}

struct PackAwareTapeBuildResult {
    tape: PackAwareTraversalTape,
    emit_positions: Vec<PackAwareTapePosition>,
    pruned_parent_expansions: usize,
}

struct PackAwareTapeBuildContext<'a> {
    repo: &'a Repository,
    walk: CommitWalkContext<'a>,
    hidden: &'a FastObjectIdSet,
    boundary_oid: Option<ObjectId>,
    frontier_since_epoch: Option<i64>,
    pack_state: &'a mut PackAwareTraversalState,
    phase_timing: &'a mut CommitWalkPhaseTiming,
}

#[derive(Debug, Clone)]
struct CachedPackTraversalMetadata {
    fingerprint: PackTraversalFingerprint,
    total_objects: usize,
    pack_paths: Vec<(PathBuf, PathBuf)>,
}

#[derive(Debug, Default)]
struct PackTraversalMetadataCache {
    by_git_dir: HashMap<PathBuf, CachedPackTraversalMetadata>,
}

struct PackAwareTraversalState {
    packs: Vec<PackBundle>,
    commit_cache: FastObjectIdMap<PackAwareCommitRecord>,
    missing_commits: FastObjectIdSet,
    decode_buffer: Vec<u8>,
    decode_inflate: gix::features::zlib::Inflate,
    decode_delta_cache: PackDecodeCache,
}

#[derive(Debug, Default)]
struct RepoPathProbeDictionary {
    next_path_id: u32,
    path_id_by_normalized_path: HashMap<String, u32>,
    probe_by_path_id: HashMap<u32, Arc<PathBloomProbePlan>>,
}

#[derive(Debug, Default)]
struct PathProbeDictionaryCache {
    by_git_dir: HashMap<PathBuf, RepoPathProbeDictionary>,
}

#[derive(Debug, Default, Clone)]
struct DenseBitmap {
    words: Vec<u64>,
}

impl DenseBitmap {
    fn mark(&mut self, index: usize) -> bool {
        let word_index = index / u64::BITS as usize;
        let bit_mask = 1u64 << (index % u64::BITS as usize);
        if word_index >= self.words.len() {
            self.words.resize(word_index + 1, 0);
        }
        if self.words[word_index] & bit_mask == 0 {
            self.words[word_index] |= bit_mask;
            true
        } else {
            false
        }
    }
}

enum PackAwareTraversalFrontier {
    BreadthFirst(VecDeque<PackAwareTapePosition>),
    DepthFirst(Vec<PackAwareTapePosition>),
}

impl PackAwareTraversalFrontier {
    fn new(sort_mode: CommitWalkSortMode, tips: Vec<PackAwareTapePosition>) -> Self {
        match sort_mode {
            CommitWalkSortMode::BreadthFirst => Self::BreadthFirst(tips.into_iter().collect()),
            CommitWalkSortMode::CommitTimeNewest | CommitWalkSortMode::CommitTimeCutoff => {
                Self::DepthFirst(tips)
            }
        }
    }

    fn pop(&mut self) -> Option<PackAwareTapePosition> {
        match self {
            Self::BreadthFirst(queue) => queue.pop_front(),
            Self::DepthFirst(stack) => stack.pop(),
        }
    }

    fn push_many(&mut self, positions: impl IntoIterator<Item = PackAwareTapePosition>) {
        match self {
            Self::BreadthFirst(queue) => queue.extend(positions),
            Self::DepthFirst(stack) => stack.extend(positions),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::BreadthFirst(queue) => queue.len(),
            Self::DepthFirst(stack) => stack.len(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangedPathDecision {
    Touched,
    NotTouched,
    Unknown,
}

pub fn validate_repository(path: &Path) -> AppResult<CanonicalRepo> {
    if !path.exists() {
        return Err(AppError::PathNotFound(path.display().to_string()));
    }

    let canonical_input = fs::canonicalize(path)?;
    let repo = gix::discover(&canonical_input)
        .map_err(|_| AppError::NotGitRepository(path.display().to_string()))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| AppError::NotGitRepository(path.display().to_string()))?;
    let root = fs::canonicalize(workdir)?;
    let git_dir = fs::canonicalize(repo.path())?;

    let name = root
        .file_name()
        .map(|segment| segment.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());

    Ok(CanonicalRepo {
        root,
        git_dir,
        name,
    })
}

pub fn load_commits(repo_root: &Path) -> AppResult<Vec<ParsedCommit>> {
    collect_commits(repo_root, None)
}

pub fn load_commits_since(repo_root: &Path, since_epoch: i64) -> AppResult<Vec<ParsedCommit>> {
    collect_commits(repo_root, Some(since_epoch.max(0)))
}

pub fn stream_commits(
    repo_root: &Path,
    since_epoch: Option<i64>,
    on_commit: impl FnMut(ParsedCommit) -> AppResult<()>,
) -> AppResult<CommitWalkExecutionStats> {
    stream_commits_with_hide(repo_root, since_epoch, None, on_commit)
}

pub fn stream_commits_with_hide(
    repo_root: &Path,
    since_epoch: Option<i64>,
    hide_ancestor_sha: Option<&str>,
    on_commit: impl FnMut(ParsedCommit) -> AppResult<()>,
) -> AppResult<CommitWalkExecutionStats> {
    walk_commits(repo_root, since_epoch, hide_ancestor_sha, on_commit)
}

fn collect_commits(repo_root: &Path, since_epoch: Option<i64>) -> AppResult<Vec<ParsedCommit>> {
    let mut commits = Vec::new();
    let _ = stream_commits(repo_root, since_epoch, |commit| {
        commits.push(commit);
        Ok(())
    })?;
    Ok(commits)
}

fn walk_commits(
    repo_root: &Path,
    since_epoch: Option<i64>,
    hide_ancestor_sha: Option<&str>,
    mut on_commit: impl FnMut(ParsedCommit) -> AppResult<()>,
) -> AppResult<CommitWalkExecutionStats> {
    let repo = open_repository(repo_root)?;
    let requested_scope = parse_commit_walk_scope();
    let tip_collection_started = Instant::now();
    let (tips, effective_scope) = collect_revision_tips(&repo, requested_scope)?;
    let tip_collection_elapsed = tip_collection_started.elapsed();
    let walk_policy = resolve_commit_walk_policy(since_epoch, hide_ancestor_sha);
    let walk_context = CommitWalkContext {
        sort_mode: walk_policy.sort_mode,
        since_epoch,
        hide_ancestor_sha,
        walk_scope: effective_scope,
        boundary: resolve_commit_walk_boundary_mode(hide_ancestor_sha),
    };
    let tip_count = tips.len();
    if tips.is_empty() {
        return Ok(CommitWalkExecutionStats {
            scope: effective_scope.as_str().to_string(),
            engine: "none".to_string(),
            sort_effective: format!(
                "{} ({})",
                walk_policy.sort_mode.as_str(),
                walk_policy.sort_reason
            ),
            boundary_mode: walk_context.boundary.as_str().to_string(),
            tip_count: 0,
            pruned_parent_expansions: 0,
            decode: Duration::ZERO,
            parent_resolve: Duration::ZERO,
            metadata_parse: Duration::ZERO,
            tip_collection: tip_collection_elapsed,
            hide_expansion: Duration::ZERO,
            graph_lookup: Duration::ZERO,
            ordering: Duration::ZERO,
            emit: Duration::ZERO,
            pack_inflate: Duration::ZERO,
            pack_base_resolve: Duration::ZERO,
            pack_parse: Duration::ZERO,
            tape_build: Duration::ZERO,
            graph_lookup_hit_count: 0,
            graph_lookup_miss_count: 0,
            graph_assist_hit_count: 0,
            graph_assist_miss_count: 0,
            nodes_seen_count: 0,
            nodes_pruned_count: 0,
            queue_max_depth: 0,
            metadata_pending_count: 0,
            graph_missing_tip_count: 0,
            boundary_stop_count: 0,
            overlap_skip_count: 0,
            tape_node_count: 0,
            tape_parent_edges: 0,
        });
    }

    let outcome = walk_commits_tape(&repo, tips, walk_context, &mut on_commit)?;

    Ok(CommitWalkExecutionStats {
        scope: effective_scope.as_str().to_string(),
        engine: outcome.engine.as_str().to_string(),
        sort_effective: format!(
            "{} ({})",
            walk_policy.sort_mode.as_str(),
            walk_policy.sort_reason
        ),
        boundary_mode: outcome.boundary_mode.as_str().to_string(),
        tip_count,
        pruned_parent_expansions: outcome.pruned_parent_expansions,
        decode: outcome.phase_timing.decode,
        parent_resolve: outcome.phase_timing.parent_resolve,
        metadata_parse: outcome.phase_timing.metadata_parse,
        tip_collection: tip_collection_elapsed,
        hide_expansion: outcome.phase_timing.hide_expansion,
        graph_lookup: outcome.phase_timing.graph_lookup,
        ordering: outcome.phase_timing.ordering,
        emit: outcome.phase_timing.emit,
        pack_inflate: outcome.phase_timing.pack_inflate,
        pack_base_resolve: outcome.phase_timing.pack_base_resolve,
        pack_parse: outcome.phase_timing.pack_parse,
        tape_build: outcome.phase_timing.tape_build,
        graph_lookup_hit_count: outcome.phase_timing.graph_lookup_hit_count,
        graph_lookup_miss_count: outcome.phase_timing.graph_lookup_miss_count,
        graph_assist_hit_count: outcome.phase_timing.graph_assist_hit_count,
        graph_assist_miss_count: outcome.phase_timing.graph_assist_miss_count,
        nodes_seen_count: outcome.phase_timing.nodes_seen_count,
        nodes_pruned_count: outcome.phase_timing.nodes_pruned_count,
        queue_max_depth: outcome.phase_timing.queue_max_depth,
        metadata_pending_count: outcome.phase_timing.metadata_pending_count,
        graph_missing_tip_count: outcome.phase_timing.graph_missing_tip_count,
        boundary_stop_count: outcome.phase_timing.boundary_stop_count,
        overlap_skip_count: outcome.phase_timing.overlap_skip_count,
        tape_node_count: outcome.phase_timing.tape_node_count,
        tape_parent_edges: outcome.phase_timing.tape_parent_edges,
    })
}

fn walk_commits_tape(
    repo: &Repository,
    tips: Vec<ObjectId>,
    context: CommitWalkContext<'_>,
    on_commit: &mut impl FnMut(ParsedCommit) -> AppResult<()>,
) -> AppResult<CommitWalkOutcome> {
    let mut graph_assist_timing = CommitWalkPhaseTiming::default();
    if let Some(graph) = load_commit_graph_cached(repo)? {
        for tip in &tips {
            if commit_graph_commit_by_id(graph.as_ref(), *tip, &mut graph_assist_timing).is_some() {
                graph_assist_timing.graph_assist_hit_count =
                    graph_assist_timing.graph_assist_hit_count.saturating_add(1);
            } else {
                graph_assist_timing.graph_assist_miss_count = graph_assist_timing
                    .graph_assist_miss_count
                    .saturating_add(1);
                graph_assist_timing.graph_missing_tip_count = graph_assist_timing
                    .graph_missing_tip_count
                    .saturating_add(1);
            }
        }
    } else {
        graph_assist_timing.graph_assist_miss_count = graph_assist_timing
            .graph_assist_miss_count
            .saturating_add(1);
    }

    let mut pack_state = build_pack_aware_traversal_state(repo)?.ok_or_else(|| {
        AppError::GitCommand("unable to initialize tape traversal state".to_string())
    })?;
    let (pruned_parent_expansions, mut phase_timing) =
        walk_commits_from_pack_frontier(repo, tips, context, &mut pack_state, on_commit)?;
    phase_timing.merge_from(graph_assist_timing);

    Ok(CommitWalkOutcome {
        engine: CommitWalkEngine::Tape,
        boundary_mode: context.boundary,
        pruned_parent_expansions,
        phase_timing,
    })
}

fn walk_commits_from_pack_frontier(
    repo: &Repository,
    tips: Vec<ObjectId>,
    context: CommitWalkContext<'_>,
    pack_state: &mut PackAwareTraversalState,
    on_commit: &mut impl FnMut(ParsedCommit) -> AppResult<()>,
) -> AppResult<(usize, CommitWalkPhaseTiming)> {
    let mut phase_timing = CommitWalkPhaseTiming::default();
    let hidden = FastObjectIdSet::default();
    let boundary_oid = if context.boundary == CommitWalkBoundaryMode::CheckpointStop {
        context
            .hide_ancestor_sha
            .map(|hide_sha| parse_hex_object_id(hide_sha, "parse pack-aware boundary commit sha"))
            .transpose()?
    } else {
        None
    };
    let frontier_since_epoch = if context.walk_scope == CommitWalkScope::Frontier {
        context.since_epoch
    } else {
        None
    };
    let PackAwareTapeBuildResult {
        tape,
        mut emit_positions,
        pruned_parent_expansions,
    } = build_pack_aware_walk_tape(
        PackAwareTapeBuildContext {
            repo,
            walk: context,
            hidden: &hidden,
            boundary_oid,
            frontier_since_epoch,
            pack_state,
            phase_timing: &mut phase_timing,
        },
        tips,
    )?;

    if context.sort_mode != CommitWalkSortMode::BreadthFirst {
        let ordering_started = Instant::now();
        emit_positions.sort_unstable_by(|left, right| {
            let left_node = &tape.nodes[*left as usize];
            let right_node = &tape.nodes[*right as usize];
            right_node
                .committer_time
                .cmp(&left_node.committer_time)
                .then_with(|| right_node.oid.cmp(&left_node.oid))
        });
        phase_timing.ordering = phase_timing
            .ordering
            .saturating_add(ordering_started.elapsed());
    }

    for position in emit_positions {
        let Some(commit) = tape.nodes.get(position as usize) else {
            continue;
        };
        phase_timing.metadata_pending_count = phase_timing.metadata_pending_count.saturating_add(1);
        let emit_started = Instant::now();
        on_commit(commit.to_pending_parsed_commit(&tape))?;
        phase_timing.emit = phase_timing.emit.saturating_add(emit_started.elapsed());
    }
    Ok((pruned_parent_expansions, phase_timing))
}

fn build_pack_aware_walk_tape(
    build: PackAwareTapeBuildContext<'_>,
    tips: Vec<ObjectId>,
) -> AppResult<PackAwareTapeBuildResult> {
    let tape_build_started = Instant::now();
    let mut tape = PackAwareTraversalTape::default();
    let tip_positions = tips
        .into_iter()
        .map(|tip| tape.ensure_position(tip))
        .collect::<Vec<_>>();
    let mut frontier = PackAwareTraversalFrontier::new(build.walk.sort_mode, tip_positions);
    let mut seen = DenseBitmap::default();
    let mut emit_positions = Vec::new();
    let mut pruned_parent_expansions = 0usize;
    build.phase_timing.queue_max_depth = build.phase_timing.queue_max_depth.max(frontier.len());

    while let Some(position) = frontier.pop() {
        if !seen.mark(position as usize) {
            continue;
        }

        let oid = tape.nodes[position as usize].oid;
        if !build.hidden.is_empty() && build.hidden.contains(&oid) {
            continue;
        }

        build.phase_timing.nodes_seen_count = build.phase_timing.nodes_seen_count.saturating_add(1);
        let parent_resolve_started = Instant::now();
        let (tree_id, committer_time, parent_positions) = {
            let Some(commit) = load_pack_aware_commit_cached(
                build.repo,
                build.pack_state,
                &oid,
                build.phase_timing,
            )?
            else {
                build.phase_timing.parent_resolve = build
                    .phase_timing
                    .parent_resolve
                    .saturating_add(parent_resolve_started.elapsed());
                continue;
            };
            let mut parent_positions =
                PackAwareTapeParentPositions::with_capacity(commit.parent_ids.len());
            for &parent_oid in &commit.parent_ids {
                parent_positions.push(tape.ensure_position(parent_oid));
            }
            (commit.tree_id, commit.committer_time, parent_positions)
        };

        let parent_count = parent_positions.len();
        let boundary_stop = build.boundary_oid == Some(oid);
        let under_since_cutoff = build
            .frontier_since_epoch
            .is_some_and(|since| committer_time < since);
        let should_emit = build
            .walk
            .since_epoch
            .is_none_or(|since| committer_time >= since);
        let should_expand = !boundary_stop && !under_since_cutoff;

        if should_expand {
            frontier.push_many(parent_positions.iter().copied());
            build.phase_timing.queue_max_depth =
                build.phase_timing.queue_max_depth.max(frontier.len());
        }

        tape.parent_edge_count = tape.parent_edge_count.saturating_add(parent_count);
        let node = &mut tape.nodes[position as usize];
        node.tree_id = tree_id;
        node.committer_time = committer_time;
        node.parent_positions = parent_positions;

        if boundary_stop {
            pruned_parent_expansions = pruned_parent_expansions.saturating_add(parent_count);
            build.phase_timing.boundary_stop_count =
                build.phase_timing.boundary_stop_count.saturating_add(1);
            build.phase_timing.parent_resolve = build
                .phase_timing
                .parent_resolve
                .saturating_add(parent_resolve_started.elapsed());
            continue;
        }

        if under_since_cutoff {
            pruned_parent_expansions = pruned_parent_expansions.saturating_add(parent_count);
            build.phase_timing.nodes_pruned_count =
                build.phase_timing.nodes_pruned_count.saturating_add(1);
            build.phase_timing.parent_resolve = build
                .phase_timing
                .parent_resolve
                .saturating_add(parent_resolve_started.elapsed());
            continue;
        }

        build.phase_timing.parent_resolve = build
            .phase_timing
            .parent_resolve
            .saturating_add(parent_resolve_started.elapsed());
        if should_emit {
            emit_positions.push(position);
        }
    }

    build.phase_timing.tape_build = build
        .phase_timing
        .tape_build
        .saturating_add(tape_build_started.elapsed());
    build.phase_timing.tape_node_count = build
        .phase_timing
        .tape_node_count
        .saturating_add(tape.nodes.len());
    build.phase_timing.tape_parent_edges = build
        .phase_timing
        .tape_parent_edges
        .saturating_add(tape.parent_edge_count);

    Ok(PackAwareTapeBuildResult {
        tape,
        emit_positions,
        pruned_parent_expansions,
    })
}

fn commit_graph_commit_by_id<'a>(
    graph: &'a gix::commitgraph::Graph,
    oid: ObjectId,
    phase_timing: &mut CommitWalkPhaseTiming,
) -> Option<gix::commitgraph::file::Commit<'a>> {
    let graph_lookup_started = Instant::now();
    let commit = graph.commit_by_id(oid);
    phase_timing.graph_lookup = phase_timing
        .graph_lookup
        .saturating_add(graph_lookup_started.elapsed());
    if commit.is_some() {
        phase_timing.graph_lookup_hit_count = phase_timing.graph_lookup_hit_count.saturating_add(1);
    } else {
        phase_timing.graph_lookup_miss_count =
            phase_timing.graph_lookup_miss_count.saturating_add(1);
    }
    commit
}

fn load_pack_aware_commit_cached<'a>(
    repo: &Repository,
    pack_state: &'a mut PackAwareTraversalState,
    oid: &ObjectId,
    phase_timing: &mut CommitWalkPhaseTiming,
) -> AppResult<Option<&'a PackAwareCommitRecord>> {
    if pack_state.commit_cache.contains_key(oid) {
        return Ok(pack_state.commit_cache.get(oid));
    }
    if pack_state.missing_commits.contains(oid) {
        return Ok(None);
    }

    if let Some(location) = locate_oid_in_packs(pack_state.packs.as_slice(), oid) {
        let decode_started = Instant::now();
        if let Some(commit) =
            decode_pack_aware_commit(repo, pack_state, oid, location, phase_timing)?
        {
            phase_timing.decode = phase_timing.decode.saturating_add(decode_started.elapsed());
            pack_state.commit_cache.insert(*oid, commit);
            return Ok(pack_state.commit_cache.get(oid));
        }
        phase_timing.decode = phase_timing.decode.saturating_add(decode_started.elapsed());
    }

    let decode_started = Instant::now();
    let commit = match repo.find_commit(*oid) {
        Ok(commit) => commit,
        Err(_) => {
            phase_timing.decode = phase_timing.decode.saturating_add(decode_started.elapsed());
            pack_state.missing_commits.insert(*oid);
            return Ok(None);
        }
    };
    phase_timing.decode = phase_timing.decode.saturating_add(decode_started.elapsed());
    let parsed = build_pack_aware_commit_from_commit(&commit);
    pack_state.commit_cache.insert(*oid, parsed);
    Ok(pack_state.commit_cache.get(oid))
}

fn locate_oid_in_packs(packs: &[PackBundle], oid: &ObjectId) -> Option<PackObjectLocation> {
    for (pack_slot, bundle) in packs.iter().enumerate() {
        if let Some(index_pos) = bundle.index.lookup(oid) {
            return Some(PackObjectLocation {
                pack_slot,
                pack_offset: bundle.index.pack_offset_at_index(index_pos),
            });
        }
    }
    None
}

fn build_pack_aware_traversal_state(
    repo: &Repository,
) -> AppResult<Option<PackAwareTraversalState>> {
    let pack_dir = repo.path().join("objects").join("pack");
    if !pack_dir.is_dir() {
        return Ok(Some(PackAwareTraversalState {
            packs: Vec::new(),
            commit_cache: FastObjectIdMap::default(),
            missing_commits: FastObjectIdSet::default(),
            decode_buffer: Vec::new(),
            decode_inflate: gix::features::zlib::Inflate::default(),
            decode_delta_cache: PackDecodeCache::new(PACK_DELTA_CACHE_BYTES),
        }));
    }

    let object_hash = repo.object_hash();
    let mut discovered_pack_paths = Vec::new();
    for dir_entry in fs::read_dir(&pack_dir)? {
        let dir_entry = dir_entry?;
        let idx_path = dir_entry.path();
        if idx_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("idx")
        {
            continue;
        }
        let pack_path = idx_path.with_extension("pack");
        if !pack_path.is_file() {
            continue;
        }
        discovered_pack_paths.push((idx_path, pack_path));
    }
    discovered_pack_paths.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    if discovered_pack_paths.is_empty() {
        return Ok(Some(PackAwareTraversalState {
            packs: Vec::new(),
            commit_cache: FastObjectIdMap::default(),
            missing_commits: FastObjectIdSet::default(),
            decode_buffer: Vec::new(),
            decode_inflate: gix::features::zlib::Inflate::default(),
            decode_delta_cache: PackDecodeCache::new(PACK_DELTA_CACHE_BYTES),
        }));
    }

    let mut fingerprint_entries = Vec::with_capacity(discovered_pack_paths.len());
    for (idx_path, pack_path) in &discovered_pack_paths {
        let idx_meta = fs::metadata(idx_path)?;
        let pack_meta = fs::metadata(pack_path)?;
        fingerprint_entries.push(PackFileFingerprint {
            idx_path: idx_path.clone(),
            idx_len: idx_meta.len(),
            idx_modified_seconds: metadata_modified_epoch_seconds(&idx_meta),
            pack_path: pack_path.clone(),
            pack_len: pack_meta.len(),
            pack_modified_seconds: metadata_modified_epoch_seconds(&pack_meta),
        });
    }
    let fingerprint = PackTraversalFingerprint {
        entries: fingerprint_entries,
    };

    let cache_key = repo_git_dir_key(repo);
    static PACK_TRAVERSAL_CACHE: OnceLock<Mutex<PackTraversalMetadataCache>> = OnceLock::new();
    let cache =
        PACK_TRAVERSAL_CACHE.get_or_init(|| Mutex::new(PackTraversalMetadataCache::default()));
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.by_git_dir.get(&cache_key) {
            if cached.fingerprint == fingerprint {
                if cached.total_objects > pack_index_max_objects() {
                    return Ok(Some(PackAwareTraversalState {
                        packs: Vec::new(),
                        commit_cache: FastObjectIdMap::default(),
                        missing_commits: FastObjectIdSet::default(),
                        decode_buffer: Vec::new(),
                        decode_inflate: gix::features::zlib::Inflate::default(),
                        decode_delta_cache: PackDecodeCache::new(PACK_DELTA_CACHE_BYTES),
                    }));
                }
                let packs =
                    open_pack_bundles_from_paths(object_hash, cached.pack_paths.as_slice())?;
                return Ok(Some(PackAwareTraversalState {
                    packs,
                    commit_cache: FastObjectIdMap::default(),
                    missing_commits: FastObjectIdSet::default(),
                    decode_buffer: Vec::new(),
                    decode_inflate: gix::features::zlib::Inflate::default(),
                    decode_delta_cache: PackDecodeCache::new(PACK_DELTA_CACHE_BYTES),
                }));
            }
        }
    }

    let mut total_objects = 0usize;
    let mut packs = Vec::with_capacity(discovered_pack_paths.len());
    let mut pack_paths = Vec::with_capacity(discovered_pack_paths.len());
    for (idx_path, pack_path) in discovered_pack_paths {
        let index = PackIndexFile::at(&idx_path, object_hash).map_err(|error| {
            AppError::GitCommand(format!(
                "open pack index for pack-aware traversal at {}: {error}",
                idx_path.display()
            ))
        })?;
        total_objects = total_objects.saturating_add(index.num_objects() as usize);
        let pack = PackDataFile::at(&pack_path, object_hash).map_err(|error| {
            AppError::GitCommand(format!(
                "open pack data for pack-aware traversal at {}: {error}",
                pack_path.display()
            ))
        })?;
        pack_paths.push((idx_path, pack_path));
        packs.push(PackBundle { index, pack });
    }

    if total_objects > pack_index_max_objects() {
        if let Ok(mut guard) = cache.lock() {
            guard.by_git_dir.insert(
                cache_key,
                CachedPackTraversalMetadata {
                    fingerprint,
                    total_objects,
                    pack_paths,
                },
            );
        }
        return Ok(Some(PackAwareTraversalState {
            packs: Vec::new(),
            commit_cache: FastObjectIdMap::default(),
            missing_commits: FastObjectIdSet::default(),
            decode_buffer: Vec::new(),
            decode_inflate: gix::features::zlib::Inflate::default(),
            decode_delta_cache: PackDecodeCache::new(PACK_DELTA_CACHE_BYTES),
        }));
    }

    if let Ok(mut guard) = cache.lock() {
        guard.by_git_dir.insert(
            cache_key,
            CachedPackTraversalMetadata {
                fingerprint,
                total_objects,
                pack_paths,
            },
        );
    }

    Ok(Some(PackAwareTraversalState {
        packs,
        commit_cache: FastObjectIdMap::default(),
        missing_commits: FastObjectIdSet::default(),
        decode_buffer: Vec::new(),
        decode_inflate: gix::features::zlib::Inflate::default(),
        decode_delta_cache: PackDecodeCache::new(PACK_DELTA_CACHE_BYTES),
    }))
}

fn open_pack_bundles_from_paths(
    object_hash: gix::hash::Kind,
    pack_paths: &[(PathBuf, PathBuf)],
) -> AppResult<Vec<PackBundle>> {
    let mut packs = Vec::with_capacity(pack_paths.len());
    for (idx_path, pack_path) in pack_paths {
        let index = PackIndexFile::at(idx_path, object_hash).map_err(|error| {
            AppError::GitCommand(format!(
                "open cached pack index for pack-aware traversal at {}: {error}",
                idx_path.display()
            ))
        })?;
        let pack = PackDataFile::at(pack_path, object_hash).map_err(|error| {
            AppError::GitCommand(format!(
                "open cached pack data for pack-aware traversal at {}: {error}",
                pack_path.display()
            ))
        })?;
        packs.push(PackBundle { index, pack });
    }
    Ok(packs)
}

fn metadata_modified_epoch_seconds(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|timestamp| timestamp.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn decode_pack_aware_commit(
    repo: &Repository,
    pack_state: &mut PackAwareTraversalState,
    oid: &ObjectId,
    location: PackObjectLocation,
    phase_timing: &mut CommitWalkPhaseTiming,
) -> AppResult<Option<PackAwareCommitRecord>> {
    let (packs, decode_buffer, decode_inflate, decode_delta_cache) = (
        &pack_state.packs,
        &mut pack_state.decode_buffer,
        &mut pack_state.decode_inflate,
        &mut pack_state.decode_delta_cache,
    );
    let Some(bundle) = packs.get(location.pack_slot) else {
        return Ok(None);
    };

    let pack_entry = bundle.pack.entry(location.pack_offset).map_err(|error| {
        AppError::GitCommand(format!(
            "read pack entry at offset {} for {}: {error}",
            location.pack_offset, oid
        ))
    })?;
    let decode_candidate = matches!(
        pack_entry.header,
        PackEntryHeader::Commit
            | PackEntryHeader::OfsDelta { .. }
            | PackEntryHeader::RefDelta { .. }
    );
    if !decode_candidate {
        return Ok(None);
    }

    decode_inflate.reset();
    decode_buffer.clear();
    let base_resolve_elapsed = Cell::new(Duration::ZERO);
    let inflate_started = Instant::now();
    let outcome = bundle
        .pack
        .decode_entry(
            pack_entry,
            decode_buffer,
            decode_inflate,
            &|id, out| {
                let base_resolve_started = Instant::now();
                let resolved = resolve_pack_base_object(repo, &bundle.index, &bundle.pack, id, out);
                base_resolve_elapsed.set(
                    base_resolve_elapsed
                        .get()
                        .saturating_add(base_resolve_started.elapsed()),
                );
                resolved
            },
            decode_delta_cache,
        )
        .map_err(|error| AppError::GitCommand(format!("decode pack entry for {}: {error}", oid)))?;
    let inflate_elapsed = inflate_started.elapsed();
    let base_resolve_elapsed = base_resolve_elapsed.get();
    phase_timing.pack_base_resolve = phase_timing
        .pack_base_resolve
        .saturating_add(base_resolve_elapsed);
    phase_timing.pack_inflate = phase_timing
        .pack_inflate
        .saturating_add(inflate_elapsed.saturating_sub(base_resolve_elapsed));
    if outcome.kind != gix::objs::Kind::Commit {
        return Ok(None);
    }

    let parse_started = Instant::now();
    let commit_ref =
        gix::objs::CommitRef::from_bytes(decode_buffer.as_slice()).map_err(|error| {
            AppError::GitCommand(format!(
                "decode commit object {} from pack data: {error}",
                oid
            ))
        })?;
    phase_timing.pack_parse = phase_timing
        .pack_parse
        .saturating_add(parse_started.elapsed());
    Ok(Some(build_pack_aware_commit_from_ref(&commit_ref)))
}

fn resolve_pack_base_object(
    repo: &Repository,
    index: &PackIndexFile,
    pack: &PackDataFile,
    id: &gix::hash::oid,
    out: &mut Vec<u8>,
) -> Option<PackResolvedBase> {
    if let Some(index_pos) = index.lookup(id) {
        let pack_offset = index.pack_offset_at_index(index_pos);
        if let Ok(entry) = pack.entry(pack_offset) {
            return Some(PackResolvedBase::InPack(entry));
        }
    }
    let object = repo.find_object(id.to_owned()).ok()?;
    out.extend_from_slice(object.data.as_slice());
    Some(PackResolvedBase::OutOfPack {
        kind: object.kind,
        end: out.len(),
    })
}

fn collect_revision_tips(
    repo: &Repository,
    requested_scope: CommitWalkScope,
) -> AppResult<(Vec<ObjectId>, CommitWalkScope)> {
    if requested_scope == CommitWalkScope::AllRefs {
        return Ok((collect_all_reference_tips(repo)?, CommitWalkScope::AllRefs));
    }

    let frontier_tips = collect_frontier_reference_tips(repo)?;
    if frontier_tips.is_empty() {
        return Ok((collect_all_reference_tips(repo)?, CommitWalkScope::AllRefs));
    }
    Ok((frontier_tips, CommitWalkScope::Frontier))
}

fn collect_all_reference_tips(repo: &Repository) -> AppResult<Vec<ObjectId>> {
    let references = repo
        .references()
        .map_err(|error| map_git_error("open reference iterator", error))?;
    let mut tips = Vec::new();
    let mut seen = HashSet::new();
    for entry in references
        .all()
        .map_err(|error| map_git_error("iterate references", error))?
    {
        let mut reference = entry.map_err(|error| map_git_error("read reference", error))?;
        let Ok(commit) = reference.peel_to_commit() else {
            continue;
        };
        let tip = commit.id().detach();
        if seen.insert(tip) {
            tips.push(tip);
        }
    }
    Ok(tips)
}

fn collect_frontier_reference_tips(repo: &Repository) -> AppResult<Vec<ObjectId>> {
    let references = repo
        .references()
        .map_err(|error| map_git_error("open frontier reference iterator", error))?;
    let iterator = references
        .prefixed("refs/heads/")
        .map_err(|error| map_git_error("iterate frontier refs/heads", error))?;

    let mut tips = Vec::new();
    let mut seen = HashSet::new();
    for entry in iterator {
        let mut reference =
            entry.map_err(|error| map_git_error("read frontier refs/heads reference", error))?;
        let Ok(commit) = reference.peel_to_commit() else {
            continue;
        };
        let tip = commit.id().detach();
        if seen.insert(tip) {
            tips.push(tip);
        }
    }

    let head_path = repo.path().join("HEAD");
    if let Ok(raw_head) = fs::read_to_string(head_path) {
        let trimmed = raw_head.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("ref:") {
            if let Ok(detached_head_oid) =
                parse_hex_object_id(trimmed, "parse detached HEAD tip for frontier scope")
            {
                if seen.insert(detached_head_oid) {
                    tips.push(detached_head_oid);
                }
            }
        }
    }

    Ok(tips)
}

fn build_ready_parsed_commit_from_commit(commit: &gix::Commit<'_>) -> ParsedCommit {
    let parents = commit.parent_ids().map(|id| id.to_string()).collect();
    let author = commit.author().ok();
    let committer = commit.committer().ok();
    let subject = commit
        .message_raw_sloppy()
        .lines()
        .next()
        .map(|line| line.to_str_lossy().into_owned())
        .unwrap_or_default();

    ParsedCommit {
        sha: commit.id().to_string(),
        parents,
        tree_sha: commit
            .tree_id()
            .map(|tree_id| tree_id.to_string())
            .unwrap_or_default(),
        author_name: author
            .map(|signature| signature.name.to_str_lossy().into_owned())
            .unwrap_or_default(),
        author_email: author
            .map(|signature| signature.email.to_str_lossy().into_owned())
            .unwrap_or_default(),
        author_time: author
            .map(|signature| signature.seconds())
            .unwrap_or_default(),
        committer_time: committer
            .map(|signature| signature.seconds())
            .unwrap_or_default(),
        subject,
        metadata_state: CommitMetadataState::Ready,
    }
}

fn build_pack_aware_commit_from_commit(commit: &gix::Commit<'_>) -> PackAwareCommitRecord {
    let parent_ids: PackAwareParentIds =
        commit.parent_ids().map(|parent| parent.detach()).collect();
    PackAwareCommitRecord {
        tree_id: commit.tree_id().ok().map(|tree_id| tree_id.detach()),
        parent_ids,
        committer_time: commit
            .committer()
            .ok()
            .map(|signature| signature.seconds())
            .unwrap_or_default(),
    }
}

fn build_pack_aware_commit_from_ref(commit: &gix::objs::CommitRef<'_>) -> PackAwareCommitRecord {
    let parent_ids: PackAwareParentIds = commit.parents().map(|parent| parent.to_owned()).collect();
    PackAwareCommitRecord {
        tree_id: Some(commit.tree().to_owned()),
        parent_ids,
        committer_time: commit
            .committer()
            .ok()
            .map(|signature| signature.seconds())
            .unwrap_or_default(),
    }
}

pub fn hydrate_commit_metadata_for_shas(
    repo_root: &Path,
    commit_shas: &[String],
) -> AppResult<(Vec<ParsedCommit>, Vec<String>)> {
    if commit_shas.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let repo = open_repository(repo_root)?;
    let mut hydrated = Vec::with_capacity(commit_shas.len());
    let mut failed = Vec::new();

    for commit_sha in commit_shas {
        let oid = match parse_hex_object_id(commit_sha, "parse commit sha for metadata hydrate") {
            Ok(oid) => oid,
            Err(_) => {
                failed.push(commit_sha.clone());
                continue;
            }
        };
        let commit = match repo.find_commit(oid) {
            Ok(commit) => commit,
            Err(_) => {
                failed.push(commit_sha.clone());
                continue;
            }
        };
        hydrated.push(build_ready_parsed_commit_from_commit(&commit));
    }

    Ok((hydrated, failed))
}

pub fn load_refs(repo_root: &Path) -> AppResult<Vec<ParsedRef>> {
    let repo = open_repository(repo_root)?;
    let mut refs = Vec::new();

    append_refs_by_glob(&repo, "refs/heads/", "head", false, &mut refs)?;
    append_refs_by_glob(&repo, "refs/tags/", "tag", true, &mut refs)?;

    Ok(refs)
}

fn append_refs_by_glob(
    repo: &Repository,
    prefix: &str,
    ref_type: &str,
    peel_to_commit: bool,
    refs: &mut Vec<ParsedRef>,
) -> AppResult<()> {
    let platform = repo
        .references()
        .map_err(|error| map_git_error("open reference iterator", error))?;
    let iterator = platform
        .prefixed(prefix)
        .map_err(|error| map_git_error(&format!("list references by prefix {prefix}"), error))?;

    for reference_result in iterator {
        let mut reference = reference_result.map_err(|error| {
            map_git_error(&format!("iterate references by prefix {prefix}"), error)
        })?;
        let Some(target_sha) = resolve_reference_target(&mut reference, peel_to_commit) else {
            continue;
        };

        refs.push(ParsedRef {
            ref_name: reference.name().to_string(),
            target_sha,
            ref_type: ref_type.to_string(),
        });
    }

    Ok(())
}

fn resolve_reference_target(
    reference: &mut gix::Reference<'_>,
    peel_to_commit: bool,
) -> Option<String> {
    if peel_to_commit {
        if let Ok(commit) = reference.peel_to_commit() {
            return Some(commit.id().to_string());
        }
    }

    if let Some(target) = reference.try_id() {
        return Some(target.to_string());
    }

    if let Ok(resolved) = reference.follow_to_object() {
        return Some(resolved.to_string());
    }

    None
}

fn parse_positive_env_usize(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn pack_index_max_objects() -> usize {
    parse_positive_env_usize(LEVEL3_PACK_INDEX_MAX_OBJECTS_ENV)
        .unwrap_or(LEVEL3_PACK_INDEX_MAX_OBJECTS_DEFAULT)
}

fn parse_env_bool_enabled(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn parse_env_bool_with_default(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        })
        .unwrap_or(default)
}

fn hot_path_rename_detection_enabled() -> bool {
    parse_env_bool_enabled(FILE_CHANGE_SYNC_STRICT_ENV)
        || parse_env_bool_enabled(FILE_CHANGE_RENAME_HOT_PATH_ENV)
}

fn parent_tree_memoization_enabled() -> bool {
    parse_env_bool_with_default(FILE_CHANGE_PARENT_TREE_MEMO_ENV, true)
}

pub fn level3_runtime_options() -> Level3RuntimeOptions {
    Level3RuntimeOptions {
        topology_aware_extraction: parse_env_bool_with_default(
            LEVEL3_TOPO_EXTRACTION_ORDER_ENV,
            true,
        ),
    }
}

fn resolve_commit_walk_boundary_mode(hide_ancestor_sha: Option<&str>) -> CommitWalkBoundaryMode {
    if hide_ancestor_sha.is_none() {
        return CommitWalkBoundaryMode::NotApplicable;
    }
    CommitWalkBoundaryMode::CheckpointStop
}

fn parse_commit_walk_sort_mode() -> Option<CommitWalkSortMode> {
    std::env::var(COMMIT_WALK_SORT_ENV).ok().and_then(|value| {
        match value.trim().to_ascii_lowercase().as_str() {
            "commit_time_newest" => Some(CommitWalkSortMode::CommitTimeNewest),
            "breadth_first" => Some(CommitWalkSortMode::BreadthFirst),
            "commit_time_cutoff" => Some(CommitWalkSortMode::CommitTimeCutoff),
            _ => None,
        }
    })
}

fn parse_commit_walk_scope() -> CommitWalkScope {
    std::env::var(COMMIT_WALK_SCOPE_ENV)
        .ok()
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "all_refs" => CommitWalkScope::AllRefs,
            _ => CommitWalkScope::Frontier,
        })
        .unwrap_or(CommitWalkScope::Frontier)
}

fn resolve_commit_walk_policy(
    since_epoch: Option<i64>,
    hide_ancestor_sha: Option<&str>,
) -> CommitWalkPolicy {
    if let Some(explicit) = parse_commit_walk_sort_mode() {
        return CommitWalkPolicy {
            sort_mode: explicit,
            sort_reason: "env_override",
        };
    }

    if since_epoch.is_some() || hide_ancestor_sha.is_some() {
        return CommitWalkPolicy {
            sort_mode: CommitWalkSortMode::CommitTimeNewest,
            sort_reason: "incremental_default_newest",
        };
    }

    CommitWalkPolicy {
        sort_mode: CommitWalkSortMode::CommitTimeNewest,
        sort_reason: "full_default",
    }
}

fn use_repo_diff_api_fallback() -> bool {
    static USE_REPO_API_DIFF: OnceLock<bool> = OnceLock::new();
    *USE_REPO_API_DIFF
        .get_or_init(|| parse_env_bool_enabled(FILE_CHANGE_DIFF_REPO_API_FALLBACK_ENV))
}

fn resolve_parallel_worker_count(
    available_threads: usize,
    configured_override: Option<usize>,
) -> usize {
    let normalized_available = available_threads.max(1);
    let default_threads = normalized_available.min(FILE_CHANGE_PARSE_WORKER_THREADS_DEFAULT_MAX);
    configured_override
        .unwrap_or(default_threads)
        .clamp(1, normalized_available)
}

fn configured_parallel_worker_count() -> usize {
    let available = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    resolve_parallel_worker_count(
        available,
        parse_positive_env_usize(FILE_CHANGE_PARSE_WORKER_THREADS_ENV),
    )
}

fn parallel_min_commits_threshold_for_workers(
    worker_count: usize,
    configured_override: Option<usize>,
) -> usize {
    let normalized_worker_count = worker_count.max(1);
    let default_threshold = FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS_DEFAULT_FLOOR.max(
        normalized_worker_count.saturating_mul(FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS_PER_WORKER),
    );
    configured_override.unwrap_or(default_threshold).max(1)
}

fn configured_parallel_min_commits_threshold(worker_count: usize) -> usize {
    parallel_min_commits_threshold_for_workers(
        worker_count,
        parse_positive_env_usize(FILE_CHANGE_PARSE_MIN_COMMITS_ENV),
    )
}

fn should_use_parallel_file_change_extraction(
    total_commits: usize,
    worker_count: usize,
    min_commits_threshold: usize,
) -> bool {
    worker_count > 1 && total_commits >= min_commits_threshold
}

fn file_change_thread_pool() -> AppResult<&'static rayon::ThreadPool> {
    static FILE_CHANGE_THREAD_POOL: OnceLock<Result<rayon::ThreadPool, String>> = OnceLock::new();

    let pool_result = FILE_CHANGE_THREAD_POOL.get_or_init(|| {
        let worker_count = configured_parallel_worker_count();
        rayon::ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .thread_name(|index| format!("gitgraph-file-change-{index}"))
            .build()
            .map_err(|error| format!("build dedicated file-change rayon thread pool: {error}"))
    });

    match pool_result {
        Ok(pool) => Ok(pool),
        Err(message) => Err(AppError::GitCommand(message.clone())),
    }
}

pub fn load_file_changes_for_commits(
    repo_root: &Path,
    commit_shas: &[String],
) -> AppResult<Vec<ParsedFileChange>> {
    let mut changes = Vec::new();
    let _telemetry = stream_file_changes_for_commits(repo_root, commit_shas, |change| {
        changes.push(change);
        Ok(())
    })?;
    Ok(changes)
}

pub(crate) struct FileChangeBatchSequentialExtractor {
    repo: Repository,
    diff_resource_cache: Option<gix::diff::blob::Platform>,
    line_stat_resource_cache: Option<gix::diff::blob::Platform>,
}

impl FileChangeBatchSequentialExtractor {
    pub(crate) fn new(repo_root: &Path) -> AppResult<Self> {
        Ok(Self {
            repo: open_repository(repo_root)?,
            diff_resource_cache: None,
            line_stat_resource_cache: None,
        })
    }

    pub(crate) fn batch_sequential_with_reused_repo(
        &mut self,
        commit_shas: &[String],
        mut on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
    ) -> AppResult<FileDiffTelemetry> {
        let mut telemetry = FileDiffTelemetry::default();
        for commit_sha in commit_shas {
            let commit_telemetry = append_file_changes_for_commit(
                &self.repo,
                commit_sha,
                FileChangeExtractionMode::MetadataOnly,
                None,
                &mut self.diff_resource_cache,
                &mut self.line_stat_resource_cache,
                &mut on_change,
            )?;
            telemetry.merge(commit_telemetry);
        }
        Ok(telemetry)
    }
}

pub fn stream_file_changes_for_commits(
    repo_root: &Path,
    commit_shas: &[String],
    on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<FileDiffTelemetry> {
    stream_file_changes_for_commits_with_profile(
        repo_root,
        commit_shas,
        FileChangeParallelProfile::Interactive,
        on_change,
    )
}

pub fn stream_file_changes_for_commits_drain(
    repo_root: &Path,
    commit_shas: &[String],
    on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<FileDiffTelemetry> {
    stream_file_changes_for_commits_with_profile(
        repo_root,
        commit_shas,
        FileChangeParallelProfile::Drain,
        on_change,
    )
}

fn stream_file_changes_for_commits_with_profile(
    repo_root: &Path,
    commit_shas: &[String],
    parallel_profile: FileChangeParallelProfile,
    mut on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<FileDiffTelemetry> {
    let mut telemetry = FileDiffTelemetry::default();
    if commit_shas.is_empty() {
        return Ok(telemetry);
    }

    let worker_count = file_change_thread_pool()?.current_num_threads().max(1);
    let min_commits_threshold = configured_parallel_min_commits_threshold(worker_count);
    if !should_use_parallel_file_change_extraction(
        commit_shas.len(),
        worker_count,
        min_commits_threshold,
    ) {
        let mut extractor = FileChangeBatchSequentialExtractor::new(repo_root)?;
        telemetry.merge(extractor.batch_sequential_with_reused_repo(commit_shas, &mut on_change)?);
        return Ok(telemetry);
    }

    let parallel_telemetry = stream_file_changes_parallel_buffered(
        repo_root,
        commit_shas,
        FileChangeExtractionMode::MetadataOnly,
        None,
        parallel_profile,
        on_change,
    )?;
    telemetry.merge(parallel_telemetry);
    Ok(telemetry)
}

fn stream_file_changes_parallel_buffered(
    repo_root: &Path,
    commit_shas: &[String],
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets_by_commit: Option<&HashMap<String, HashSet<String>>>,
    parallel_profile: FileChangeParallelProfile,
    mut on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<FileDiffTelemetry> {
    let mut total_telemetry = FileDiffTelemetry::default();
    let _preflight_repo = open_repository(repo_root)?;
    let pool = file_change_thread_pool()?;
    let worker_count = pool.current_num_threads().max(1);
    let parallel_plan =
        parallel_file_change_plan(commit_shas.len(), worker_count, parallel_profile);
    let base_chunks: Vec<&[String]> = commit_shas
        .chunks(FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE)
        .collect();
    let commit_chunks: Vec<&[String]> =
        if parallel_plan.chunk_size > FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE {
            commit_shas.chunks(parallel_plan.chunk_size).collect()
        } else if parallel_plan.chunk_size < FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE {
            base_chunks
                .into_iter()
                .flat_map(|chunk| chunk.chunks(parallel_plan.chunk_size))
                .collect()
        } else {
            base_chunks
        };
    let total_chunks = commit_chunks.len();
    if total_chunks == 0 {
        return Ok(total_telemetry);
    }

    let line_stat_targets_by_commit = line_stat_targets_by_commit.cloned();
    let mut chunk_start = 0usize;
    while chunk_start < total_chunks {
        let chunk_end = (chunk_start + parallel_plan.wave_chunk_limit).min(total_chunks);
        let wave_chunks = &commit_chunks[chunk_start..chunk_end];
        if wave_chunks.is_empty() {
            break;
        }

        let channel_capacity = parallel_plan.channel_capacity.min(wave_chunks.len()).max(1);
        let (sender, receiver) =
            sync_channel::<(usize, AppResult<ChunkFileChanges>)>(channel_capacity);
        let mut ordered_wave_results: Vec<Option<ChunkFileChanges>> =
            std::iter::repeat_with(|| None)
                .take(wave_chunks.len())
                .collect();
        let mut first_error: Option<AppError> = None;

        std::thread::scope(|scope| -> AppResult<()> {
            let producer_repo_root = repo_root.to_path_buf();
            let line_stat_targets_by_commit = line_stat_targets_by_commit.clone();
            scope.spawn(move || {
                pool.install(|| {
                    wave_chunks
                        .into_par_iter()
                        .enumerate()
                        .map_init(
                            || open_repository(&producer_repo_root),
                            |repo_state, (chunk_index, chunk)| {
                                let chunk_result = match repo_state {
                                    Ok(repo) => collect_file_changes_for_chunk_with_repo(
                                        repo,
                                        chunk,
                                        extraction_mode,
                                        line_stat_targets_by_commit.as_ref(),
                                    ),
                                    Err(error) => Err(AppError::GitCommand(format!(
                                        "initialize parallel file-change extraction worker repository handle: {error}"
                                    ))),
                                };
                                (chunk_index, chunk_result)
                            },
                        )
                        .for_each_with(sender, |tx, (chunk_index, chunk_result)| {
                            let _ = tx.send((chunk_index, chunk_result));
                        });
                });
            });

            let mut received = 0usize;
            while received < wave_chunks.len() {
                let received_chunk = receiver.recv().map_err(|_| {
                    AppError::GitCommand(
                        "receive parsed file-change chunk from parallel producer".to_string(),
                    )
                });
                match received_chunk {
                    Ok((chunk_index, chunk_result)) => {
                        received = received.saturating_add(1);
                        match chunk_result {
                            Ok(chunk) => {
                                if first_error.is_none() {
                                    if chunk_index < ordered_wave_results.len() {
                                        ordered_wave_results[chunk_index] = Some(chunk);
                                    } else {
                                        first_error = Some(AppError::Parse(format!(
                                            "parallel file-change chunk index out of range: {chunk_index}"
                                        )));
                                    }
                                }
                            }
                            Err(error) => {
                                if first_error.is_none() {
                                    first_error = Some(error);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                        break;
                    }
                }
            }

            Ok(())
        })?;

        if let Some(error) = first_error {
            return Err(error);
        }

        for maybe_chunk in ordered_wave_results {
            let chunk = maybe_chunk.ok_or_else(|| {
                AppError::GitCommand(
                    "missing ordered parallel file-change chunk result".to_string(),
                )
            })?;
            total_telemetry.merge(chunk.telemetry);
            for change in chunk.changes {
                on_change(change)?;
            }
        }

        chunk_start = chunk_end;
    }

    Ok(total_telemetry)
}

fn collect_file_changes_for_chunk_with_repo(
    repo: &Repository,
    commit_shas: &[String],
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets_by_commit: Option<&HashMap<String, HashSet<String>>>,
) -> AppResult<ChunkFileChanges> {
    let mut telemetry = FileDiffTelemetry::default();
    let mut chunk_changes = Vec::new();
    if parent_tree_memoization_enabled()
        && extraction_mode == FileChangeExtractionMode::MetadataOnly
        && commit_shas.len() >= FILE_CHANGE_PARENT_TREE_MEMO_MIN_COMMITS
    {
        let mut grouped: BTreeMap<String, Vec<(usize, &String)>> = BTreeMap::new();
        for (index, commit_sha) in commit_shas.iter().enumerate() {
            let signature = parent_tree_signature(repo, commit_sha)?;
            grouped
                .entry(signature)
                .or_default()
                .push((index, commit_sha));
        }

        let mut ordered_telemetry = vec![FileDiffTelemetry::default(); commit_shas.len()];
        let mut ordered_changes: Vec<Vec<ParsedFileChange>> = vec![Vec::new(); commit_shas.len()];
        for (_signature, commits) in grouped {
            let mut diff_resource_cache = None;
            let mut line_stat_resource_cache = None;
            for (index, commit_sha) in commits {
                let line_stat_targets = line_stat_targets_by_commit
                    .and_then(|targets_by_commit| targets_by_commit.get(commit_sha));
                let commit_telemetry = append_file_changes_for_commit(
                    repo,
                    commit_sha,
                    extraction_mode,
                    line_stat_targets,
                    &mut diff_resource_cache,
                    &mut line_stat_resource_cache,
                    &mut |change| {
                        ordered_changes[index].push(change);
                        Ok(())
                    },
                )?;
                ordered_telemetry[index] = commit_telemetry;
            }
        }

        for (commit_telemetry, mut changes) in ordered_telemetry
            .into_iter()
            .zip(ordered_changes.into_iter())
        {
            telemetry.merge(commit_telemetry);
            chunk_changes.append(&mut changes);
        }
    } else {
        let mut diff_resource_cache = None;
        let mut line_stat_resource_cache = None;
        for commit_sha in commit_shas {
            let line_stat_targets = line_stat_targets_by_commit
                .and_then(|targets_by_commit| targets_by_commit.get(commit_sha));
            let commit_telemetry = append_file_changes_for_commit(
                repo,
                commit_sha,
                extraction_mode,
                line_stat_targets,
                &mut diff_resource_cache,
                &mut line_stat_resource_cache,
                &mut |change| {
                    chunk_changes.push(change);
                    Ok(())
                },
            )?;
            telemetry.merge(commit_telemetry);
        }
    }

    Ok(ChunkFileChanges {
        changes: chunk_changes,
        telemetry,
    })
}

fn parent_tree_signature(repo: &Repository, commit_sha: &str) -> AppResult<String> {
    let oid = parse_hex_object_id(commit_sha, "parse commit sha for parent-tree signature")?;
    let commit = repo
        .find_commit(oid)
        .map_err(|error| map_git_error("resolve commit for parent-tree signature", error))?;
    let commit_tree = commit
        .tree_id()
        .ok()
        .map(|tree| tree.detach().to_string())
        .unwrap_or_else(|| "missing-commit-tree".to_string());

    let parent_tree = commit
        .parent_ids()
        .next()
        .map(|parent_id| {
            repo.find_commit(parent_id.detach())
                .map_err(|error| map_git_error("resolve parent commit for signature", error))
                .and_then(|parent| {
                    parent
                        .tree_id()
                        .ok()
                        .map(|tree| tree.detach().to_string())
                        .ok_or_else(|| {
                            AppError::GitCommand(
                                "resolve parent tree for parent-tree signature".to_string(),
                            )
                        })
                })
        })
        .transpose()?
        .unwrap_or_else(|| "root".to_string());

    Ok(format!("{parent_tree}|{commit_tree}"))
}

pub fn reorder_commits_topologically(
    repo_root: &Path,
    commit_shas: &[String],
) -> AppResult<Vec<String>> {
    if commit_shas.len() <= 1 {
        return Ok(commit_shas.to_vec());
    }

    let repo = open_repository(repo_root)?;
    let mut parents_by_sha: HashMap<String, Vec<String>> =
        HashMap::with_capacity(commit_shas.len());
    for sha in commit_shas {
        let oid = parse_hex_object_id(sha, "parse commit sha for topology ordering")?;
        let commit = repo
            .find_commit(oid)
            .map_err(|error| map_git_error("resolve commit for topology ordering", error))?;
        parents_by_sha.insert(
            sha.clone(),
            commit
                .parent_ids()
                .map(|parent| parent.to_string())
                .collect(),
        );
    }
    Ok(reorder_commits_topologically_with_parents(
        commit_shas,
        &parents_by_sha,
    ))
}

pub fn reorder_commits_topologically_with_parents(
    commit_shas: &[String],
    parents_by_sha: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    if commit_shas.len() <= 1 {
        return commit_shas.to_vec();
    }

    let wanted: HashSet<&str> = commit_shas.iter().map(String::as_str).collect();
    let mut index_by_sha: HashMap<&str, usize> = HashMap::with_capacity(commit_shas.len());
    for (position, sha) in commit_shas.iter().enumerate() {
        index_by_sha.insert(sha.as_str(), position);
    }

    let mut indegree: HashMap<String, usize> = HashMap::with_capacity(commit_shas.len());
    let mut children: HashMap<String, Vec<String>> = HashMap::with_capacity(commit_shas.len());
    for sha in commit_shas {
        let parents = parents_by_sha
            .get(sha.as_str())
            .map(|parents| parents.as_slice())
            .unwrap_or(&[]);
        let mut parent_count = 0usize;
        for parent_sha in parents {
            if wanted.contains(parent_sha.as_str()) {
                parent_count = parent_count.saturating_add(1);
                children
                    .entry(parent_sha.clone())
                    .or_default()
                    .push(sha.clone());
            }
        }
        indegree.insert(sha.clone(), parent_count);
        children.entry(sha.clone()).or_default();
    }

    let mut ready = BinaryHeap::new();
    for sha in commit_shas {
        if indegree.get(sha.as_str()).copied().unwrap_or(0) == 0 {
            let position = index_by_sha
                .get(sha.as_str())
                .copied()
                .unwrap_or(usize::MAX);
            ready.push(Reverse((position, sha.clone())));
        }
    }

    let mut queue = VecDeque::new();
    while let Some(item) = ready.pop() {
        queue.push_back(item.0 .1);
    }

    let mut ordered = Vec::with_capacity(commit_shas.len());
    while let Some(sha) = queue.pop_front() {
        ordered.push(sha.clone());
        if let Some(next_children) = children.get(sha.as_str()) {
            for child_sha in next_children {
                if let Some(entry) = indegree.get_mut(child_sha.as_str()) {
                    if *entry > 0 {
                        *entry -= 1;
                    }
                    if *entry == 0 {
                        let position = index_by_sha
                            .get(child_sha.as_str())
                            .copied()
                            .unwrap_or(usize::MAX);
                        ready.push(Reverse((position, child_sha.clone())));
                    }
                }
            }
            while let Some(item) = ready.pop() {
                queue.push_back(item.0 .1);
            }
        }
    }

    if ordered.len() == commit_shas.len() {
        ordered
    } else {
        commit_shas.to_vec()
    }
}

fn commit_might_touch_target_paths(
    repo: &Repository,
    commit_sha: &str,
    target_paths: &HashSet<String>,
    cached_index: &Option<Arc<CommitGraphBloomIndex>>,
) -> AppResult<bool> {
    if target_paths.is_empty() {
        return Ok(false);
    }

    let Ok(oid) = parse_hex_object_id(commit_sha, "parse commit sha for changed-path bloom lookup")
    else {
        return Ok(true);
    };
    Ok(matches!(
        evaluate_changed_path_decision_for_oid(repo, &oid, target_paths, cached_index)?,
        ChangedPathDecision::Touched | ChangedPathDecision::Unknown
    ))
}

fn evaluate_changed_path_decision_for_oid(
    repo: &Repository,
    commit_oid: &ObjectId,
    target_paths: &HashSet<String>,
    cached_index: &Option<Arc<CommitGraphBloomIndex>>,
) -> AppResult<ChangedPathDecision> {
    if target_paths.is_empty() {
        return Ok(ChangedPathDecision::NotTouched);
    }

    let Some(index) = cached_index else {
        return Ok(ChangedPathDecision::Unknown);
    };
    let Some(filter) = lookup_bloom_filter_for_commit(index, commit_oid) else {
        return Ok(ChangedPathDecision::Unknown);
    };

    let prehashed_plans = load_prehashed_path_probe_plans_cached(repo, target_paths);
    if prehashed_plans.is_empty() {
        for path in target_paths {
            if bloom_filter_might_contain_path(filter, path) {
                return Ok(ChangedPathDecision::Touched);
            }
        }
    } else {
        for plan in &prehashed_plans {
            if bloom_filter_might_contain_prehashed_path(filter, plan.as_ref()) {
                return Ok(ChangedPathDecision::Touched);
            }
        }
    }

    if commit_touches_target_paths_exact(repo, commit_oid, target_paths)? {
        Ok(ChangedPathDecision::Touched)
    } else {
        Ok(ChangedPathDecision::NotTouched)
    }
}

fn load_prehashed_path_probe_plans_cached(
    repo: &Repository,
    target_paths: &HashSet<String>,
) -> Vec<Arc<PathBloomProbePlan>> {
    if target_paths.is_empty() {
        return Vec::new();
    }

    static PATH_PROBE_CACHE: OnceLock<Mutex<PathProbeDictionaryCache>> = OnceLock::new();
    let cache = PATH_PROBE_CACHE.get_or_init(|| Mutex::new(PathProbeDictionaryCache::default()));
    let git_dir = repo_git_dir_key(repo);

    let mut normalized_paths = Vec::with_capacity(target_paths.len());
    let mut seen = HashSet::with_capacity(target_paths.len());
    for raw_path in target_paths {
        let normalized = normalize_repo_relative_path(raw_path);
        if normalized.is_empty() || !seen.insert(normalized.clone()) {
            continue;
        }
        normalized_paths.push(normalized);
    }
    if normalized_paths.is_empty() {
        return Vec::new();
    }

    if let Ok(mut guard) = cache.lock() {
        let dictionary = guard.by_git_dir.entry(git_dir).or_default();
        let mut plans = Vec::with_capacity(normalized_paths.len());
        for normalized in normalized_paths {
            if let Some(plan) = get_or_insert_path_probe_plan(dictionary, normalized.as_str()) {
                plans.push(plan);
            }
        }
        return plans;
    }

    normalized_paths
        .into_iter()
        .filter_map(|normalized| build_path_bloom_probe_plan(normalized.as_str()).map(Arc::new))
        .collect()
}

fn get_or_insert_path_probe_plan(
    dictionary: &mut RepoPathProbeDictionary,
    normalized_path: &str,
) -> Option<Arc<PathBloomProbePlan>> {
    let path_id = if let Some(existing) = dictionary
        .path_id_by_normalized_path
        .get(normalized_path)
        .copied()
    {
        existing
    } else {
        let assigned = dictionary.next_path_id;
        dictionary.next_path_id = dictionary.next_path_id.saturating_add(1);
        dictionary
            .path_id_by_normalized_path
            .insert(normalized_path.to_string(), assigned);
        assigned
    };

    if let Some(existing) = dictionary.probe_by_path_id.get(&path_id) {
        return Some(existing.clone());
    }

    let plan = Arc::new(build_path_bloom_probe_plan(normalized_path)?);
    dictionary.probe_by_path_id.insert(path_id, plan.clone());
    Some(plan)
}

fn build_path_bloom_probe_plan(normalized_path: &str) -> Option<PathBloomProbePlan> {
    if normalized_path.is_empty() {
        return None;
    }

    let bytes = normalized_path.as_bytes();
    let mut prehashed_keys =
        Vec::with_capacity(1 + bytes.iter().filter(|byte| **byte == b'/').count());
    prehashed_keys.push(prehash_bloom_probe_key(bytes));
    for split in (0..bytes.len()).rev() {
        if bytes[split] == b'/' && split > 0 {
            prehashed_keys.push(prehash_bloom_probe_key(&bytes[..split]));
        }
    }

    Some(PathBloomProbePlan { prehashed_keys })
}

fn prehash_bloom_probe_key(key: &[u8]) -> PrehashedBloomProbe {
    PrehashedBloomProbe {
        hash0_v1: bloom_murmur3_seeded(BLOOM_HASH_VERSION_GIT_V1, BLOOM_MURMUR_SEED0, key),
        hash1_v1: bloom_murmur3_seeded(BLOOM_HASH_VERSION_GIT_V1, BLOOM_MURMUR_SEED1, key),
        hash0_v2: bloom_murmur3_seeded(BLOOM_HASH_VERSION_GIT_V2, BLOOM_MURMUR_SEED0, key),
        hash1_v2: bloom_murmur3_seeded(BLOOM_HASH_VERSION_GIT_V2, BLOOM_MURMUR_SEED1, key),
    }
}

fn repo_git_dir_key(repo: &Repository) -> PathBuf {
    fs::canonicalize(repo.path()).unwrap_or_else(|_| repo.path().to_path_buf())
}

fn load_commit_graph_bloom_index_cached(
    repo: &Repository,
) -> AppResult<Option<Arc<CommitGraphBloomIndex>>> {
    static BLOOM_CACHE: OnceLock<Mutex<CommitGraphBloomCache>> = OnceLock::new();
    let cache = BLOOM_CACHE.get_or_init(|| Mutex::new(CommitGraphBloomCache::default()));
    let git_dir = repo_git_dir_key(repo);

    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.by_git_dir.get(&git_dir) {
            return Ok(cached.clone());
        }
    }

    let loaded = load_commit_graph_bloom_index(repo).map(Arc::new);
    if let Ok(mut guard) = cache.lock() {
        guard.by_git_dir.insert(git_dir, loaded.clone());
    }
    Ok(loaded)
}

fn load_commit_graph_cached(repo: &Repository) -> AppResult<Option<Arc<gix::commitgraph::Graph>>> {
    static GRAPH_CACHE: OnceLock<Mutex<CommitGraphMetadataCache>> = OnceLock::new();
    let cache = GRAPH_CACHE.get_or_init(|| Mutex::new(CommitGraphMetadataCache::default()));
    let git_dir = repo_git_dir_key(repo);

    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.by_git_dir.get(&git_dir) {
            return Ok(cached.clone());
        }
    }

    let info_dir = repo.path().join("objects").join("info");
    let loaded = gix::commitgraph::Graph::from_info_dir(info_dir.as_path())
        .ok()
        .map(Arc::new);
    if let Ok(mut guard) = cache.lock() {
        guard.by_git_dir.insert(git_dir, loaded.clone());
    }
    Ok(loaded)
}

fn load_commit_graph_bloom_index(repo: &Repository) -> Option<CommitGraphBloomIndex> {
    let info_dir = repo.path().join("objects").join("info");
    if !info_dir.is_dir() {
        return None;
    }

    let commit_graphs_dir = info_dir.join("commit-graphs");
    let chain_file = commit_graphs_dir.join("commit-graph-chain");
    let graph_files: Vec<PathBuf> = if chain_file.is_file() {
        let chain_content = fs::read_to_string(chain_file).ok()?;
        chain_content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|hash| commit_graphs_dir.join(format!("graph-{hash}.graph")))
            .collect()
    } else {
        let monolithic = info_dir.join("commit-graph");
        if monolithic.is_file() {
            vec![monolithic]
        } else {
            Vec::new()
        }
    };

    if graph_files.is_empty() {
        return None;
    }

    let hash_len = repo.object_hash().len_in_bytes();
    let mut layers = Vec::new();
    for path in graph_files {
        let Some(layer) = parse_commit_graph_bloom_layer(path.as_path(), hash_len) else {
            continue;
        };
        layers.push(layer);
    }

    if layers.is_empty() {
        None
    } else {
        Some(CommitGraphBloomIndex { layers })
    }
}

fn parse_commit_graph_bloom_layer(
    path: &Path,
    expected_hash_len: usize,
) -> Option<CommitGraphBloomLayer> {
    let data = fs::read(path).ok()?;
    if data.len() < COMMIT_GRAPH_CHUNK_LOOKUP_HEADER_LEN {
        return None;
    }
    if data.get(0..4)? != COMMIT_GRAPH_SIGNATURE {
        return None;
    }
    if data[4] != 1 {
        return None;
    }

    let hash_len = match data[5] {
        1 => 20usize,
        2 => 32usize,
        _ => return None,
    };
    if hash_len != expected_hash_len {
        return None;
    }

    let chunk_count = usize::from(data[6]);
    let lookup_len = (chunk_count + 1).checked_mul(COMMIT_GRAPH_CHUNK_LOOKUP_ENTRY_LEN)?;
    let lookup_start = COMMIT_GRAPH_CHUNK_LOOKUP_HEADER_LEN;
    let lookup_end = lookup_start.checked_add(lookup_len)?;
    if lookup_end > data.len() {
        return None;
    }

    let mut chunk_ids = Vec::with_capacity(chunk_count + 1);
    let mut chunk_offsets = Vec::with_capacity(chunk_count + 1);
    for index in 0..=chunk_count {
        let entry_start = lookup_start + index * COMMIT_GRAPH_CHUNK_LOOKUP_ENTRY_LEN;
        let id: [u8; 4] = data.get(entry_start..entry_start + 4)?.try_into().ok()?;
        let offset_u64 = u64::from_be_bytes(
            data.get(entry_start + 4..entry_start + 12)?
                .try_into()
                .ok()?,
        );
        let offset = usize::try_from(offset_u64).ok()?;
        chunk_ids.push(id);
        chunk_offsets.push(offset);
    }

    let mut chunk_ranges: HashMap<[u8; 4], (usize, usize)> = HashMap::new();
    for index in 0..chunk_count {
        let id = chunk_ids[index];
        if id == [0, 0, 0, 0] {
            continue;
        }
        let start = chunk_offsets[index];
        let end = chunk_offsets[index + 1];
        if start > end || end > data.len() {
            return None;
        }
        chunk_ranges.insert(id, (start, end));
    }

    let (oid_fan_offset, oid_fan_end) = *chunk_ranges.get(&COMMIT_GRAPH_CHUNK_ID_OIDF)?;
    if oid_fan_end.saturating_sub(oid_fan_offset) != COMMIT_GRAPH_FANOUT_CHUNK_LEN {
        return None;
    }

    let commit_count =
        usize::try_from(read_be_u32(data.as_slice(), oid_fan_offset + (255 * 4))?).ok()?;
    let (oid_lookup_offset, oid_lookup_end) = *chunk_ranges.get(&COMMIT_GRAPH_CHUNK_ID_OIDL)?;
    let oid_lookup_len = oid_lookup_end.saturating_sub(oid_lookup_offset);
    if oid_lookup_len < commit_count.saturating_mul(hash_len) {
        return None;
    }

    let (bloom_index_offset, bloom_index_end) = *chunk_ranges.get(&COMMIT_GRAPH_CHUNK_ID_BIDX)?;
    if bloom_index_end.saturating_sub(bloom_index_offset) != commit_count.saturating_mul(4) {
        return None;
    }

    let (bloom_data_offset, bloom_data_end) = *chunk_ranges.get(&COMMIT_GRAPH_CHUNK_ID_BDAT)?;
    if bloom_data_end <= bloom_data_offset + BLOOM_DATA_HEADER_LEN {
        return None;
    }

    let bloom_hash_version = read_be_u32(data.as_slice(), bloom_data_offset)?;
    let bloom_num_hashes = read_be_u32(data.as_slice(), bloom_data_offset + 4)?;
    let _bloom_bits_per_entry = read_be_u32(data.as_slice(), bloom_data_offset + 8)?;
    if bloom_num_hashes == 0 {
        return None;
    }
    if bloom_hash_version != BLOOM_HASH_VERSION_GIT_V1
        && bloom_hash_version != BLOOM_HASH_VERSION_GIT_V2
    {
        return None;
    }

    let bloom_payload_len = bloom_data_end
        .saturating_sub(bloom_data_offset)
        .saturating_sub(BLOOM_DATA_HEADER_LEN);
    let mut previous = 0usize;
    for index in 0..commit_count {
        let end = usize::try_from(read_be_u32(
            data.as_slice(),
            bloom_index_offset + index * 4,
        )?)
        .ok()?;
        if end < previous || end > bloom_payload_len {
            return None;
        }
        previous = end;
    }

    Some(CommitGraphBloomLayer {
        data,
        oid_fan_offset,
        oid_lookup_offset,
        commit_count,
        hash_len,
        bloom_index_offset,
        bloom_data_offset,
        bloom_data_end,
        bloom_hash_version,
        bloom_num_hashes,
    })
}

fn lookup_bloom_filter_for_commit<'a>(
    index: &'a CommitGraphBloomIndex,
    oid: &ObjectId,
) -> Option<BloomFilterView<'a>> {
    let oid_bytes = oid.as_slice();
    for layer in &index.layers {
        if layer.hash_len != oid_bytes.len() {
            continue;
        }
        let Some(lex_pos) = lookup_commit_lex_position(layer, oid_bytes) else {
            continue;
        };
        let Some(filter) = bloom_filter_for_lex_position(layer, lex_pos) else {
            continue;
        };
        return Some(filter);
    }
    None
}

fn lookup_commit_lex_position(layer: &CommitGraphBloomLayer, oid_bytes: &[u8]) -> Option<usize> {
    let first_byte = usize::from(*oid_bytes.first()?);
    let lower = if first_byte == 0 {
        0usize
    } else {
        usize::try_from(read_be_u32(
            layer.data.as_slice(),
            layer.oid_fan_offset + (first_byte - 1) * 4,
        )?)
        .ok()?
    };
    let mut upper = usize::try_from(read_be_u32(
        layer.data.as_slice(),
        layer.oid_fan_offset + first_byte * 4,
    )?)
    .ok()?;
    let mut lower = lower;
    while lower < upper {
        let mid = lower + (upper - lower) / 2;
        let oid_start = layer.oid_lookup_offset + mid * layer.hash_len;
        let oid_end = oid_start + layer.hash_len;
        let probe = layer.data.get(oid_start..oid_end)?;
        match probe.cmp(oid_bytes) {
            std::cmp::Ordering::Less => lower = mid.saturating_add(1),
            std::cmp::Ordering::Greater => upper = mid,
            std::cmp::Ordering::Equal => return Some(mid),
        }
    }
    None
}

fn bloom_filter_for_lex_position<'a>(
    layer: &'a CommitGraphBloomLayer,
    lex_pos: usize,
) -> Option<BloomFilterView<'a>> {
    if lex_pos >= layer.commit_count {
        return None;
    }
    let end = usize::try_from(read_be_u32(
        layer.data.as_slice(),
        layer.bloom_index_offset + lex_pos * 4,
    )?)
    .ok()?;
    let start = if lex_pos == 0 {
        0usize
    } else {
        usize::try_from(read_be_u32(
            layer.data.as_slice(),
            layer.bloom_index_offset + (lex_pos - 1) * 4,
        )?)
        .ok()?
    };
    if end < start {
        return None;
    }

    let payload_start = layer.bloom_data_offset + BLOOM_DATA_HEADER_LEN;
    let payload_end = layer.bloom_data_end;
    let filter_start = payload_start.saturating_add(start);
    let filter_end = payload_start.saturating_add(end);
    if filter_end > payload_end || filter_start > filter_end {
        return None;
    }
    let bytes = layer.data.get(filter_start..filter_end)?;
    Some(BloomFilterView {
        bytes,
        hash_version: layer.bloom_hash_version,
        num_hashes: layer.bloom_num_hashes,
    })
}

fn bloom_filter_might_contain_path(filter: BloomFilterView<'_>, path: &str) -> bool {
    let normalized = normalize_repo_relative_path(path);
    if normalized.is_empty() {
        return false;
    }
    let bytes = normalized.as_bytes();
    if bloom_filter_contains_key(filter, bytes) {
        return true;
    }

    for split in (0..bytes.len()).rev() {
        if bytes[split] == b'/' && split > 0 && bloom_filter_contains_key(filter, &bytes[..split]) {
            return true;
        }
    }
    false
}

fn bloom_filter_might_contain_prehashed_path(
    filter: BloomFilterView<'_>,
    plan: &PathBloomProbePlan,
) -> bool {
    for key in &plan.prehashed_keys {
        if bloom_filter_contains_prehashed_key(filter, *key) {
            return true;
        }
    }
    false
}

fn bloom_filter_contains_prehashed_key(
    filter: BloomFilterView<'_>,
    prehashed: PrehashedBloomProbe,
) -> bool {
    if filter.num_hashes == 0 || filter.bytes.is_empty() {
        return false;
    }
    let total_bits = filter.bytes.len().saturating_mul(8);
    if total_bits == 0 {
        return false;
    }

    let (hash0, hash1) = if filter.hash_version == BLOOM_HASH_VERSION_GIT_V2 {
        (prehashed.hash0_v2, prehashed.hash1_v2)
    } else {
        (prehashed.hash0_v1, prehashed.hash1_v1)
    };

    let rounds = usize::try_from(filter.num_hashes).unwrap_or(0);
    for round in 0..rounds {
        let hash = hash0.wrapping_add((round as u32).wrapping_mul(hash1));
        let bit_pos = usize::try_from(hash).unwrap_or(0) % total_bits;
        let byte = filter.bytes[bit_pos / 8];
        let mask = 1u8 << (bit_pos & 7);
        if byte & mask == 0 {
            return false;
        }
    }
    true
}

fn bloom_filter_contains_key(filter: BloomFilterView<'_>, key: &[u8]) -> bool {
    if key.is_empty() || filter.num_hashes == 0 || filter.bytes.is_empty() {
        return false;
    }
    let total_bits = filter.bytes.len().saturating_mul(8);
    if total_bits == 0 {
        return false;
    }

    let hash0 = bloom_murmur3_seeded(filter.hash_version, BLOOM_MURMUR_SEED0, key);
    let hash1 = bloom_murmur3_seeded(filter.hash_version, BLOOM_MURMUR_SEED1, key);
    let rounds = usize::try_from(filter.num_hashes).unwrap_or(0);
    for round in 0..rounds {
        let hash = hash0.wrapping_add((round as u32).wrapping_mul(hash1));
        let bit_pos = usize::try_from(hash).unwrap_or(0) % total_bits;
        let byte = filter.bytes[bit_pos / 8];
        let mask = 1u8 << (bit_pos & 7);
        if byte & mask == 0 {
            return false;
        }
    }
    true
}

fn bloom_murmur3_seeded(hash_version: u32, seed: u32, data: &[u8]) -> u32 {
    if hash_version == BLOOM_HASH_VERSION_GIT_V2 {
        return bloom_murmur3_seeded_v2(seed, data);
    }
    bloom_murmur3_seeded_v1(seed, data)
}

fn bloom_murmur3_seeded_v2(mut seed: u32, data: &[u8]) -> u32 {
    const C1: u32 = 0xcc9e2d51;
    const C2: u32 = 0x1b873593;
    const M: u32 = 5;
    const N: u32 = 0xe6546b64;

    let mut chunks = data.chunks_exact(4);
    for chunk in chunks.by_ref() {
        let mut k = u32::from(chunk[0])
            | (u32::from(chunk[1]) << 8)
            | (u32::from(chunk[2]) << 16)
            | (u32::from(chunk[3]) << 24);
        k = k.wrapping_mul(C1);
        k = k.rotate_left(15);
        k = k.wrapping_mul(C2);
        seed ^= k;
        seed = seed.rotate_left(13).wrapping_mul(M).wrapping_add(N);
    }

    let tail = chunks.remainder();
    let mut k1 = 0u32;
    if tail.len() == 3 {
        k1 ^= u32::from(tail[2]) << 16;
    }
    if tail.len() >= 2 {
        k1 ^= u32::from(tail[1]) << 8;
    }
    if !tail.is_empty() {
        k1 ^= u32::from(tail[0]);
        k1 = k1.wrapping_mul(C1);
        k1 = k1.rotate_left(15);
        k1 = k1.wrapping_mul(C2);
        seed ^= k1;
    }

    seed ^= u32::try_from(data.len()).unwrap_or(u32::MAX);
    seed ^= seed >> 16;
    seed = seed.wrapping_mul(0x85ebca6b);
    seed ^= seed >> 13;
    seed = seed.wrapping_mul(0xc2b2ae35);
    seed ^= seed >> 16;
    seed
}

fn bloom_murmur3_seeded_v1(mut seed: u32, data: &[u8]) -> u32 {
    const C1: u32 = 0xcc9e2d51;
    const C2: u32 = 0x1b873593;
    const M: u32 = 5;
    const N: u32 = 0xe6546b64;

    let mut chunks = data.chunks_exact(4);
    for chunk in chunks.by_ref() {
        let mut k = ((chunk[0] as i8 as i32) as u32)
            | (((chunk[1] as i8 as i32) as u32) << 8)
            | (((chunk[2] as i8 as i32) as u32) << 16)
            | (((chunk[3] as i8 as i32) as u32) << 24);
        k = k.wrapping_mul(C1);
        k = k.rotate_left(15);
        k = k.wrapping_mul(C2);
        seed ^= k;
        seed = seed.rotate_left(13).wrapping_mul(M).wrapping_add(N);
    }

    let tail = chunks.remainder();
    let mut k1 = 0u32;
    if tail.len() == 3 {
        k1 ^= ((tail[2] as i8 as i32) as u32) << 16;
    }
    if tail.len() >= 2 {
        k1 ^= ((tail[1] as i8 as i32) as u32) << 8;
    }
    if !tail.is_empty() {
        k1 ^= (tail[0] as i8 as i32) as u32;
        k1 = k1.wrapping_mul(C1);
        k1 = k1.rotate_left(15);
        k1 = k1.wrapping_mul(C2);
        seed ^= k1;
    }

    seed ^= u32::try_from(data.len()).unwrap_or(u32::MAX);
    seed ^= seed >> 16;
    seed = seed.wrapping_mul(0x85ebca6b);
    seed ^= seed >> 13;
    seed = seed.wrapping_mul(0xc2b2ae35);
    seed ^= seed >> 16;
    seed
}

fn commit_touches_target_paths_exact(
    repo: &Repository,
    commit_oid: &ObjectId,
    target_paths: &HashSet<String>,
) -> AppResult<bool> {
    if target_paths.is_empty() {
        return Ok(false);
    }

    let commit = match repo.find_commit(*commit_oid) {
        Ok(commit) => commit,
        Err(_) => return Ok(true),
    };
    let commit_tree = match commit.tree() {
        Ok(tree) => tree,
        Err(_) => return Ok(true),
    };
    let parent_tree = commit
        .parent_ids()
        .next()
        .map(|parent_id| {
            repo.find_commit(parent_id.detach())
                .map_err(|error| {
                    map_git_error("load parent commit for exact changed-path check", error)
                })
                .and_then(|parent| {
                    parent.tree().map_err(|error| {
                        map_git_error("load parent tree for exact changed-path check", error)
                    })
                })
        })
        .transpose()
        .unwrap_or(None);

    for raw_path in target_paths {
        let normalized_path = normalize_repo_relative_path(raw_path);
        if normalized_path.is_empty() {
            continue;
        }
        let path = Path::new(normalized_path.as_str());
        let commit_entry = match commit_tree.lookup_entry_by_path(path) {
            Ok(value) => value,
            Err(_) => return Ok(true),
        };
        let parent_entry = match parent_tree.as_ref() {
            Some(parent_tree) => match parent_tree.lookup_entry_by_path(path) {
                Ok(value) => value,
                Err(_) => return Ok(true),
            },
            None => None,
        };
        if tree_entry_fingerprint(commit_entry.as_ref())
            != tree_entry_fingerprint(parent_entry.as_ref())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn tree_entry_fingerprint(entry: Option<&gix::object::tree::Entry<'_>>) -> Option<(u16, ObjectId)> {
    entry.map(|value| (value.mode().value(), value.object_id()))
}

fn normalize_repo_relative_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    while normalized.starts_with('/') {
        normalized.remove(0);
    }
    normalized
}

fn read_be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn bloom_filter_indicates_no_first_parent_changes(filter: BloomFilterView<'_>) -> bool {
    !filter.bytes.is_empty() && filter.bytes.iter().all(|byte| *byte == 0)
}

fn commit_tree_matches_first_parent_in_commit_graph(
    repo: &Repository,
    commit_oid: &ObjectId,
) -> AppResult<Option<bool>> {
    let Some(graph) = load_commit_graph_cached(repo)? else {
        return Ok(None);
    };
    let Some(commit) = graph.commit_by_id(commit_oid) else {
        return Ok(None);
    };
    let first_parent = match commit.parent1() {
        Ok(parent) => parent,
        Err(_) => return Ok(None),
    };
    let Some(parent_pos) = first_parent else {
        return Ok(Some(false));
    };
    let parent = graph.commit_at(parent_pos);
    Ok(Some(parent.root_tree_id() == commit.root_tree_id()))
}

fn commit_might_have_first_parent_changes(
    commit_oid: &ObjectId,
    cached_index: &Option<Arc<CommitGraphBloomIndex>>,
) -> AppResult<bool> {
    let Some(index) = cached_index else {
        return Ok(true);
    };
    let Some(filter) = lookup_bloom_filter_for_commit(index, commit_oid) else {
        return Ok(true);
    };
    Ok(!bloom_filter_indicates_no_first_parent_changes(filter))
}

pub fn filter_commits_for_line_stat_targets(
    repo_root: &Path,
    commit_shas: &[String],
    line_stat_targets_by_commit: &HashMap<String, HashSet<String>>,
) -> AppResult<Vec<String>> {
    if commit_shas.is_empty() {
        return Ok(Vec::new());
    }

    let repo = match open_repository(repo_root) {
        Ok(repo) => repo,
        Err(_) => return Ok(commit_shas.to_vec()),
    };
    let shared_bloom_index = load_commit_graph_bloom_index_cached(&repo)?;
    let mut filtered = Vec::with_capacity(commit_shas.len());
    for sha in commit_shas {
        let Some(target_paths) = line_stat_targets_by_commit.get(sha) else {
            continue;
        };
        if commit_might_touch_target_paths(&repo, sha, target_paths, &shared_bloom_index)? {
            filtered.push(sha.clone());
        }
    }
    Ok(filtered)
}

pub fn load_file_change_line_stats_for_targets(
    repo_root: &Path,
    targets: &[PendingFileChangeEnrichment],
) -> AppResult<Vec<FileChangeLineStatsUpdate>> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }

    let mut line_stat_targets_by_commit: HashMap<String, HashSet<String>> = HashMap::new();
    for target in targets {
        line_stat_targets_by_commit
            .entry(target.commit_sha.clone())
            .or_default()
            .insert(target.file_path.clone());
    }

    let mut commit_shas: Vec<String> = line_stat_targets_by_commit.keys().cloned().collect();
    commit_shas.sort_unstable();
    let worker_count = file_change_thread_pool()?.current_num_threads().max(1);
    let min_commits_threshold = configured_parallel_min_commits_threshold(worker_count);
    let parsed = if commit_shas.is_empty() {
        Vec::new()
    } else if !should_use_parallel_file_change_extraction(
        commit_shas.len(),
        worker_count,
        min_commits_threshold,
    ) {
        let repo = open_repository(repo_root)?;
        let mut diff_resource_cache = None;
        let mut changes = Vec::new();
        let mut line_stat_resource_cache = None;
        for commit_sha in &commit_shas {
            let line_stat_targets = line_stat_targets_by_commit.get(commit_sha);
            let _commit_telemetry = append_file_changes_for_commit(
                &repo,
                commit_sha,
                FileChangeExtractionMode::SelectiveLineStats,
                line_stat_targets,
                &mut diff_resource_cache,
                &mut line_stat_resource_cache,
                &mut |change| {
                    changes.push(change);
                    Ok(())
                },
            )?;
        }
        changes
    } else {
        let mut changes = Vec::new();
        let _telemetry = stream_file_changes_parallel_buffered(
            repo_root,
            &commit_shas,
            FileChangeExtractionMode::SelectiveLineStats,
            Some(&line_stat_targets_by_commit),
            FileChangeParallelProfile::Interactive,
            |change| {
                changes.push(change);
                Ok(())
            },
        )?;
        changes
    };

    let mut by_commit: HashMap<String, HashMap<String, (i64, i64)>> = HashMap::new();
    for change in parsed {
        let additions = change.additions.unwrap_or_default();
        let deletions = change.deletions.unwrap_or_default();
        by_commit
            .entry(change.commit_sha)
            .or_default()
            .insert(change.file_path, (additions, deletions));
    }

    let mut updates = Vec::with_capacity(targets.len());
    for target in targets {
        let (additions, deletions) = by_commit
            .get(&target.commit_sha)
            .and_then(|files| files.get(&target.file_path))
            .copied()
            .unwrap_or_default();
        updates.push(FileChangeLineStatsUpdate {
            commit_sha: target.commit_sha.clone(),
            file_path: target.file_path.clone(),
            additions,
            deletions,
        });
    }

    Ok(updates)
}

fn rename_detection_policy(
    delta_count: usize,
    max_file_size_bytes: u64,
) -> Option<RenameDetectionPolicy> {
    if delta_count == 0 {
        return None;
    }

    let rename_threshold = rename_threshold_for_bins(delta_count, max_file_size_bytes);
    Some(RenameDetectionPolicy {
        rename_limit: delta_count.clamp(1, rename_limit_for_bins(delta_count)),
        rename_threshold,
        exact_match_only: delta_count >= FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS
            || max_file_size_bytes >= FILE_CHANGE_EXACT_MATCH_ONLY_FILE_BYTES,
    })
}

fn rename_limit_for_bins(delta_count: usize) -> usize {
    if delta_count >= FILE_CHANGE_RENAME_LIMIT_XLARGE_MIN_DELTAS {
        return parse_positive_env_usize(FILE_CHANGE_RENAME_LIMIT_XLARGE_ENV)
            .unwrap_or(FILE_CHANGE_RENAME_LIMIT_XLARGE_DEFAULT)
            .clamp(1, FILE_CHANGE_RENAME_LIMIT);
    }

    if delta_count >= FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS {
        return parse_positive_env_usize(FILE_CHANGE_RENAME_LIMIT_LARGE_ENV)
            .unwrap_or(FILE_CHANGE_RENAME_LIMIT_LARGE_DEFAULT)
            .clamp(1, FILE_CHANGE_RENAME_LIMIT);
    }

    FILE_CHANGE_RENAME_LIMIT
}

fn rename_threshold_for_bins(delta_count: usize, max_file_size_bytes: u64) -> u16 {
    if delta_count >= FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS
        || max_file_size_bytes >= FILE_CHANGE_RENAME_LARGE_FILE_BYTES
    {
        return FILE_CHANGE_RENAME_THRESHOLD_LARGE;
    }

    if delta_count >= 128 || max_file_size_bytes >= FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES {
        return FILE_CHANGE_RENAME_THRESHOLD_MEDIUM;
    }

    FILE_CHANGE_RENAME_THRESHOLD_SMALL
}

fn should_run_rename_detection(
    extraction_mode: FileChangeExtractionMode,
    delta_count: usize,
    has_add_delete_pair: bool,
) -> bool {
    if delta_count < 2 {
        return false;
    }

    if extraction_mode == FileChangeExtractionMode::MetadataOnly
        && !hot_path_rename_detection_enabled()
    {
        return false;
    }

    match extraction_mode {
        FileChangeExtractionMode::MetadataOnly => has_add_delete_pair,
        FileChangeExtractionMode::SelectiveLineStats => {
            has_add_delete_pair || delta_count >= FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS
        }
    }
}

fn drain_parallel_chunk_size() -> usize {
    parse_positive_env_usize(FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE_ENV)
        .unwrap_or(FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE_DEFAULT)
        .clamp(
            FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE,
            FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE_MAX,
        )
}

fn drain_parallel_channel_capacity() -> usize {
    parse_positive_env_usize(FILE_CHANGE_DRAIN_CHANNEL_CAPACITY_ENV)
        .unwrap_or(FILE_CHANGE_DRAIN_CHANNEL_CAPACITY_DEFAULT)
        .clamp(1, FILE_CHANGE_DRAIN_CHANNEL_CAPACITY_MAX)
}

pub fn configured_file_change_parallel_worker_count() -> usize {
    configured_parallel_worker_count()
}

pub fn configured_file_change_drain_parallel_chunk_size() -> usize {
    drain_parallel_chunk_size()
}

pub fn configured_file_change_drain_channel_capacity() -> usize {
    drain_parallel_channel_capacity()
}

fn parallel_file_change_plan(
    total_commits: usize,
    worker_count: usize,
    profile: FileChangeParallelProfile,
) -> ParallelFileChangePlan {
    if profile == FileChangeParallelProfile::Drain {
        let chunk_size = drain_parallel_chunk_size();
        let channel_capacity = drain_parallel_channel_capacity();
        return ParallelFileChangePlan {
            chunk_size,
            channel_capacity,
            wave_chunk_limit: channel_capacity.max(1),
        };
    }

    let worker_count = worker_count.max(1);
    let target_chunk_count =
        worker_count.saturating_mul(FILE_CHANGE_PARSE_PARALLEL_TARGET_CHUNKS_PER_WORKER);
    let adaptive_chunk_size = if target_chunk_count == 0 {
        FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE
    } else {
        total_commits.div_ceil(target_chunk_count)
    };
    let chunk_size = adaptive_chunk_size.clamp(
        FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE,
        FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE,
    );
    let channel_capacity = worker_count
        .saturating_mul(FILE_CHANGE_PARSE_CHANNEL_CAPACITY_PER_WORKER)
        .clamp(
            FILE_CHANGE_PARSE_CHANNEL_CAPACITY,
            FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX,
        );
    let wave_chunk_limit = channel_capacity.max(1);

    ParallelFileChangePlan {
        chunk_size,
        channel_capacity,
        wave_chunk_limit,
    }
}

fn rewrites_from_policy(policy: RenameDetectionPolicy) -> Rewrites {
    Rewrites {
        copies: None,
        percentage: if policy.exact_match_only {
            None
        } else {
            Some((f32::from(policy.rename_threshold) / 100.0).clamp(0.0, 1.0))
        },
        limit: policy.rename_limit,
        track_empty: false,
    }
}

fn diff_requires_line_stats(delta: &ChangeDetached) -> bool {
    if !change_is_blob_or_symlink(delta) {
        return false;
    }

    match delta {
        ChangeDetached::Addition { .. }
        | ChangeDetached::Deletion { .. }
        | ChangeDetached::Modification { .. } => true,
        ChangeDetached::Rewrite { source_id, id, .. } => source_id != id,
    }
}

fn change_is_blob_or_symlink(delta: &ChangeDetached) -> bool {
    match delta {
        ChangeDetached::Addition { entry_mode, .. }
        | ChangeDetached::Deletion { entry_mode, .. } => entry_mode.is_blob_or_symlink(),
        ChangeDetached::Modification {
            previous_entry_mode,
            entry_mode,
            ..
        } => previous_entry_mode.is_blob_or_symlink() || entry_mode.is_blob_or_symlink(),
        ChangeDetached::Rewrite {
            source_entry_mode,
            entry_mode,
            ..
        } => source_entry_mode.is_blob_or_symlink() || entry_mode.is_blob_or_symlink(),
    }
}

fn change_is_binary(delta: &ChangeDetached) -> bool {
    !change_is_blob_or_symlink(delta)
}

fn parse_hex_object_id(object_id: &str, context: &str) -> AppResult<ObjectId> {
    let oid_len = object_id.len();
    if oid_len != MIN_OBJECT_ID_HEX_LEN
        || !object_id
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::InvalidArgument(format!(
            "{context}: expected {MIN_OBJECT_ID_HEX_LEN} hex chars"
        )));
    }

    ObjectId::from_hex(object_id.as_bytes()).map_err(|error| map_git_error(context, error))
}

fn object_size_bytes(repo: &Repository, id: ObjectId) -> u64 {
    repo.try_find_header(id)
        .ok()
        .flatten()
        .map(|header| header.size())
        .unwrap_or(0)
}

fn diff_has_add_delete_pair(diff: &[ChangeDetached]) -> bool {
    let mut has_added = false;
    let mut has_deleted = false;

    for delta in diff {
        match delta {
            ChangeDetached::Addition { .. } => {
                has_added = true;
            }
            ChangeDetached::Deletion { .. } => {
                has_deleted = true;
            }
            ChangeDetached::Modification { .. } | ChangeDetached::Rewrite { .. } => {}
        }
        if has_added && has_deleted {
            return true;
        }
    }

    false
}

fn diff_max_file_size_bytes(repo: &Repository, diff: &[ChangeDetached]) -> u64 {
    let mut max_file_size_bytes = 0u64;
    for delta in diff {
        match delta {
            ChangeDetached::Addition { id, .. } | ChangeDetached::Deletion { id, .. } => {
                max_file_size_bytes = max_file_size_bytes.max(object_size_bytes(repo, *id));
            }
            ChangeDetached::Modification {
                previous_id, id, ..
            } => {
                max_file_size_bytes =
                    max_file_size_bytes.max(object_size_bytes(repo, *previous_id));
                max_file_size_bytes = max_file_size_bytes.max(object_size_bytes(repo, *id));
            }
            ChangeDetached::Rewrite { source_id, id, .. } => {
                max_file_size_bytes = max_file_size_bytes.max(object_size_bytes(repo, *source_id));
                max_file_size_bytes = max_file_size_bytes.max(object_size_bytes(repo, *id));
            }
        }
        if max_file_size_bytes >= FILE_CHANGE_EXACT_MATCH_ONLY_FILE_BYTES {
            break;
        }
    }
    max_file_size_bytes
}

fn should_probe_max_file_size(delta_count: usize) -> bool {
    delta_count < FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS
}

fn get_or_init_tree_diff_resource_cache<'a>(
    repo: &Repository,
    diff_resource_cache: &'a mut Option<gix::diff::blob::Platform>,
) -> AppResult<&'a mut gix::diff::blob::Platform> {
    if diff_resource_cache.is_none() {
        *diff_resource_cache = Some(
            repo.diff_resource_cache_for_tree_diff()
                .map_err(|error| map_git_error("prepare tree-diff resource cache", error))?,
        );
    }
    diff_resource_cache
        .as_mut()
        .ok_or_else(|| AppError::GitCommand("initialize tree-diff resource cache".to_string()))
}

fn diff_commit_trees(
    repo: &Repository,
    parent_tree: Option<&gix::Tree<'_>>,
    commit_tree: &gix::Tree<'_>,
    rewrite_options: Option<Rewrites>,
    diff_resource_cache: &mut Option<gix::diff::blob::Platform>,
) -> AppResult<Vec<ChangeDetached>> {
    let mut diff_options = gix::diff::Options::default();
    diff_options.track_rewrites(rewrite_options);

    if use_repo_diff_api_fallback() {
        return repo
            .diff_tree_to_tree(parent_tree, Some(commit_tree), Some(diff_options))
            .map_err(|error| map_git_error("compute tree diff for commit", error));
    }

    let empty_tree = repo.empty_tree();
    let base_tree = parent_tree.unwrap_or(&empty_tree);
    let cache = get_or_init_tree_diff_resource_cache(repo, diff_resource_cache)?;

    let mut platform = base_tree
        .changes()
        .map_err(|error| map_git_error("prepare tree-diff platform for commit", error))?;
    platform.options(|options| {
        *options = diff_options;
    });

    let mut out = Vec::new();
    platform
        .for_each_to_obtain_tree_with_cache(commit_tree, cache, |change| {
            out.push(change.detach());
            Ok::<gix::object::tree::diff::Action, std::convert::Infallible>(
                gix::object::tree::diff::Action::Continue(()),
            )
        })
        .map_err(|error| map_git_error("compute tree diff for commit", error))?;
    cache.clear_resource_cache_keep_allocation();
    Ok(out)
}

fn get_or_init_line_stat_resource_cache<'a>(
    repo: &Repository,
    line_stat_resource_cache: &'a mut Option<gix::diff::blob::Platform>,
) -> AppResult<&'a mut gix::diff::blob::Platform> {
    if line_stat_resource_cache.is_none() {
        *line_stat_resource_cache = Some(
            repo.diff_resource_cache_for_tree_diff()
                .map_err(|error| map_git_error("prepare selective line-stat diff cache", error))?,
        );
    }
    line_stat_resource_cache.as_mut().ok_or_else(|| {
        AppError::GitCommand("initialize selective line-stat resource cache".to_string())
    })
}

fn collect_patch_line_stats(
    repo: &Repository,
    resource_cache: &mut gix::diff::blob::Platform,
    diff: &[ChangeDetached],
    diff_index: usize,
) -> AppResult<Option<(i64, i64)>> {
    let _ = PATCH_FROM_DIFF_GUARDRAIL_MARKER;
    let Some(change) = diff.get(diff_index) else {
        return Ok(None);
    };
    if !diff_requires_line_stats(change) {
        return Ok(None);
    }

    let mut platform = change
        .attach(repo, repo)
        .diff(resource_cache)
        .map_err(|error| map_git_error("create patch for selective line stats", error))?;
    let counts = platform
        .line_counts()
        .map_err(|error| map_git_error("read patch line stats", error))?;
    resource_cache.clear_resource_cache_keep_allocation();
    Ok(counts.map(|counter| {
        (
            usize_to_i64_saturating(counter.insertions as usize),
            usize_to_i64_saturating(counter.removals as usize),
        )
    }))
}

fn should_collect_targeted_line_stats(
    extraction_mode: FileChangeExtractionMode,
    delta: &ChangeDetached,
    path: &str,
    line_stat_targets: Option<&HashSet<String>>,
) -> bool {
    extraction_mode == FileChangeExtractionMode::SelectiveLineStats
        && diff_requires_line_stats(delta)
        && line_stat_targets
            .map(|targets| targets.contains(path))
            .unwrap_or(false)
}

fn should_emit_path(
    extraction_mode: FileChangeExtractionMode,
    path: &str,
    filter_paths: Option<&HashSet<String>>,
) -> bool {
    extraction_mode != FileChangeExtractionMode::SelectiveLineStats
        || filter_paths
            .map(|targets| targets.contains(path))
            .unwrap_or(false)
}

fn should_skip_commit_with_fused_pre_diff_gate(
    repo: &Repository,
    commit_oid: &ObjectId,
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets: Option<&HashSet<String>>,
) -> AppResult<bool> {
    if extraction_mode == FileChangeExtractionMode::SelectiveLineStats {
        let Some(target_paths) = line_stat_targets else {
            return Ok(true);
        };
        if target_paths.is_empty() {
            return Ok(true);
        }
        let shared_bloom_index = load_commit_graph_bloom_index_cached(repo)?;
        return Ok(matches!(
            evaluate_changed_path_decision_for_oid(
                repo,
                commit_oid,
                target_paths,
                &shared_bloom_index,
            )?,
            ChangedPathDecision::NotTouched
        ));
    }

    if matches!(
        commit_tree_matches_first_parent_in_commit_graph(repo, commit_oid)?,
        Some(true)
    ) {
        return Ok(true);
    }
    let shared_bloom_index = load_commit_graph_bloom_index_cached(repo)?;
    if !commit_might_have_first_parent_changes(commit_oid, &shared_bloom_index)? {
        return Ok(true);
    }

    Ok(false)
}

fn append_file_changes_for_commit(
    repo: &Repository,
    commit_sha: &str,
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets: Option<&HashSet<String>>,
    diff_resource_cache: &mut Option<gix::diff::blob::Platform>,
    line_stat_resource_cache: &mut Option<gix::diff::blob::Platform>,
    on_change: &mut impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<FileDiffTelemetry> {
    let mut telemetry = FileDiffTelemetry::default();
    let oid = parse_hex_object_id(commit_sha, "parse commit sha")?;
    if should_skip_commit_with_fused_pre_diff_gate(repo, &oid, extraction_mode, line_stat_targets)?
    {
        return Ok(telemetry);
    }

    let commit = repo
        .find_commit(oid)
        .map_err(|error| map_git_error("resolve commit for file change extraction", error))?;
    let first_parent_id = commit
        .parent_ids()
        .next()
        .map(|parent_id| parent_id.detach());
    let first_parent_commit = first_parent_id
        .map(|parent_id| {
            repo.find_commit(parent_id)
                .map_err(|error| map_git_error("read first parent commit", error))
        })
        .transpose()?;

    if extraction_mode == FileChangeExtractionMode::MetadataOnly {
        if let Some(parent_commit) = first_parent_commit.as_ref() {
            let commit_tree_id = commit.tree_id().ok().map(|id| id.detach());
            let parent_tree_id = parent_commit.tree_id().ok().map(|id| id.detach());
            if commit_tree_id.is_some() && commit_tree_id == parent_tree_id {
                return Ok(telemetry);
            }
        }
    }

    let diff_build_started = Instant::now();
    let commit_tree = commit
        .tree()
        .map_err(|error| map_git_error("read commit tree", error))?;
    let parent_tree = first_parent_commit
        .as_ref()
        .map(|parent| {
            parent
                .tree()
                .map_err(|error| map_git_error("read parent commit tree", error))
        })
        .transpose()?;

    let raw_diff = diff_commit_trees(
        repo,
        parent_tree.as_ref(),
        &commit_tree,
        None,
        diff_resource_cache,
    )?;
    telemetry.diff_build = telemetry
        .diff_build
        .saturating_add(diff_build_started.elapsed());

    let rename_find_started = Instant::now();
    let delta_count = raw_diff.len();
    let mut diff = raw_diff;
    if delta_count >= 2 {
        let has_add_delete_pair = diff_has_add_delete_pair(&diff);
        if should_run_rename_detection(extraction_mode, delta_count, has_add_delete_pair) {
            let max_file_size_bytes = if should_probe_max_file_size(delta_count) {
                diff_max_file_size_bytes(repo, &diff)
            } else {
                0
            };
            if let Some(rename_policy) = rename_detection_policy(delta_count, max_file_size_bytes) {
                diff = diff_commit_trees(
                    repo,
                    parent_tree.as_ref(),
                    &commit_tree,
                    Some(rewrites_from_policy(rename_policy)),
                    diff_resource_cache,
                )
                .map_err(|error| map_git_error("detect file renames in commit diff", error))?;
            }
        }
    }
    telemetry.rename_find = telemetry
        .rename_find
        .saturating_add(rename_find_started.elapsed());

    let delta_emit_started = Instant::now();
    if diff.len() <= 1 {
        emit_single_delta_fast_path(
            repo,
            &diff,
            commit_sha,
            extraction_mode,
            line_stat_targets,
            line_stat_resource_cache,
            on_change,
        )?;
        telemetry.delta_emit = telemetry
            .delta_emit
            .saturating_add(delta_emit_started.elapsed());
        return Ok(telemetry);
    }

    let mut by_path: HashMap<String, FileChangeAccumulator> = HashMap::new();
    let mut ordered_paths: Vec<String> = Vec::new();
    let mut selected_line_stat_targets: Vec<DiffLineStatTarget> = Vec::new();

    for (diff_index, delta) in diff.iter().enumerate() {
        let path = delta_display_path(delta);
        if path.is_empty() {
            continue;
        }

        match by_path.entry(path.clone()) {
            Entry::Vacant(entry) => {
                ordered_paths.push(path.clone());
                entry.insert(FileChangeAccumulator::from_delta(delta));
            }
            Entry::Occupied(mut entry) => {
                entry.get_mut().apply_delta(delta);
            }
        }

        if should_collect_targeted_line_stats(
            extraction_mode,
            delta,
            path.as_str(),
            line_stat_targets,
        ) {
            selected_line_stat_targets.push(DiffLineStatTarget { diff_index, path });
        }
    }

    if !selected_line_stat_targets.is_empty() {
        let resource_cache = get_or_init_line_stat_resource_cache(repo, line_stat_resource_cache)?;
        for target in selected_line_stat_targets {
            let Some((additions, deletions)) =
                collect_patch_line_stats(repo, resource_cache, &diff, target.diff_index)?
            else {
                continue;
            };
            if let Some(accumulator) = by_path.get_mut(&target.path) {
                accumulator.add_line_stats(additions, deletions);
            }
        }
    }

    let filter_paths = line_stat_targets;
    let emit_selected_paths_only = extraction_mode == FileChangeExtractionMode::SelectiveLineStats;
    if emit_selected_paths_only && filter_paths.is_none() {
        telemetry.delta_emit = telemetry
            .delta_emit
            .saturating_add(delta_emit_started.elapsed());
        return Ok(telemetry);
    }

    for path in ordered_paths {
        if !should_emit_path(extraction_mode, path.as_str(), filter_paths) {
            continue;
        }

        let Some(accumulator) = by_path.remove(&path) else {
            continue;
        };
        let (additions, deletions, stats_state, requires_line_stats) = match extraction_mode {
            FileChangeExtractionMode::MetadataOnly => accumulator.metadata_view(),
            FileChangeExtractionMode::SelectiveLineStats => accumulator.line_stats_view(),
        };
        on_change(ParsedFileChange {
            commit_sha: commit_sha.to_string(),
            file_path: path,
            additions,
            deletions,
            change_type: accumulator.change_type,
            stats_state,
            requires_line_stats,
        })?;
    }

    telemetry.delta_emit = telemetry
        .delta_emit
        .saturating_add(delta_emit_started.elapsed());
    Ok(telemetry)
}

fn emit_single_delta_fast_path(
    repo: &Repository,
    diff: &[ChangeDetached],
    commit_sha: &str,
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets: Option<&HashSet<String>>,
    line_stat_resource_cache: &mut Option<gix::diff::blob::Platform>,
    on_change: &mut impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<()> {
    let Some(delta) = diff.first() else {
        return Ok(());
    };

    let path = delta_display_path(delta);
    if path.is_empty() {
        return Ok(());
    }

    if !should_emit_path(extraction_mode, path.as_str(), line_stat_targets) {
        return Ok(());
    }

    let mut accumulator = FileChangeAccumulator::from_delta(delta);
    if should_collect_targeted_line_stats(extraction_mode, delta, path.as_str(), line_stat_targets)
    {
        let resource_cache = get_or_init_line_stat_resource_cache(repo, line_stat_resource_cache)?;
        if let Some((additions, deletions)) =
            collect_patch_line_stats(repo, resource_cache, diff, 0)?
        {
            accumulator.add_line_stats(additions, deletions);
        }
    }

    let (additions, deletions, stats_state, requires_line_stats) = match extraction_mode {
        FileChangeExtractionMode::MetadataOnly => accumulator.metadata_view(),
        FileChangeExtractionMode::SelectiveLineStats => accumulator.line_stats_view(),
    };
    on_change(ParsedFileChange {
        commit_sha: commit_sha.to_string(),
        file_path: path,
        additions,
        deletions,
        change_type: accumulator.change_type,
        stats_state,
        requires_line_stats,
    })?;

    Ok(())
}

fn delta_display_path(delta: &ChangeDetached) -> String {
    match delta {
        ChangeDetached::Rewrite {
            source_location,
            location,
            ..
        } => {
            let old_path = String::from_utf8_lossy(source_location.as_ref()).into_owned();
            let new_path = String::from_utf8_lossy(location.as_ref()).into_owned();
            if !old_path.is_empty() && !new_path.is_empty() && old_path != new_path {
                format!("{old_path} => {new_path}")
            } else if !new_path.is_empty() {
                new_path
            } else {
                old_path
            }
        }
        ChangeDetached::Addition { location, .. }
        | ChangeDetached::Deletion { location, .. }
        | ChangeDetached::Modification { location, .. } => {
            String::from_utf8_lossy(location.as_ref()).into_owned()
        }
    }
}

fn delta_change_type(delta: &ChangeDetached) -> String {
    match delta {
        ChangeDetached::Addition { .. } => "add".to_string(),
        ChangeDetached::Deletion { .. } => "delete".to_string(),
        ChangeDetached::Rewrite { copy, .. } if !copy => "rename".to_string(),
        _ => "modify".to_string(),
    }
}

fn open_repository(repo_root: &Path) -> AppResult<Repository> {
    let mut repo = gix::open(repo_root.to_path_buf())
        .map_err(|error| map_git_error("open git repository", error))?;
    let worker_concurrency_hint = if rayon::current_thread_index().is_some() {
        rayon::current_num_threads().max(1)
    } else {
        1
    };
    repo.object_cache_size_if_unset(object_cache_bytes_for_workers(worker_concurrency_hint));
    Ok(repo)
}

fn object_cache_bytes_for_workers(worker_count: usize) -> usize {
    let normalized_worker_count = worker_count.max(1);
    if normalized_worker_count == 1 {
        return GIX_OBJECT_CACHE_BYTES_DEFAULT;
    }

    GIX_OBJECT_CACHE_BYTES_DEFAULT
        .saturating_div(normalized_worker_count)
        .clamp(GIX_OBJECT_CACHE_BYTES_MIN, GIX_OBJECT_CACHE_BYTES_MAX)
}

fn map_git_error(context: &str, error: impl std::fmt::Display) -> AppError {
    AppError::GitCommand(format!("{context}: {error}"))
}

fn usize_to_i64_saturating(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[derive(Debug, Clone)]
struct FileChangeAccumulator {
    additions: i64,
    deletions: i64,
    change_type: String,
    binary: bool,
    requires_line_stats: bool,
}

impl FileChangeAccumulator {
    fn from_delta(delta: &ChangeDetached) -> Self {
        let binary = change_is_binary(delta);
        Self {
            additions: 0,
            deletions: 0,
            change_type: if binary {
                "binary".to_string()
            } else {
                delta_change_type(delta)
            },
            binary,
            requires_line_stats: diff_requires_line_stats(delta),
        }
    }

    fn apply_delta(&mut self, delta: &ChangeDetached) {
        self.requires_line_stats = self.requires_line_stats || diff_requires_line_stats(delta);
        self.binary = self.binary || change_is_binary(delta);
        if self.binary {
            self.change_type = "binary".to_string();
        } else if self.change_type == "modify" {
            self.change_type = delta_change_type(delta);
        }
    }

    fn add_line_stats(&mut self, additions: i64, deletions: i64) {
        self.additions = self.additions.saturating_add(additions);
        self.deletions = self.deletions.saturating_add(deletions);
    }

    fn metadata_view(&self) -> (Option<i64>, Option<i64>, FileChangeStatsState, bool) {
        if self.requires_line_stats {
            return (None, None, FileChangeStatsState::Pending, true);
        }

        (
            Some(self.additions),
            Some(self.deletions),
            FileChangeStatsState::Ready,
            false,
        )
    }

    fn line_stats_view(&self) -> (Option<i64>, Option<i64>, FileChangeStatsState, bool) {
        (
            Some(self.additions),
            Some(self.deletions),
            FileChangeStatsState::Ready,
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bloom_filter_contains_key, bloom_filter_indicates_no_first_parent_changes,
        bloom_murmur3_seeded, filter_commits_for_line_stat_targets, level3_runtime_options,
        normalize_repo_relative_path, object_cache_bytes_for_workers, parallel_file_change_plan,
        parallel_min_commits_threshold_for_workers, parse_commit_walk_scope, parse_hex_object_id,
        rename_detection_policy, rename_limit_for_bins, rename_threshold_for_bins,
        reorder_commits_topologically_with_parents, resolve_commit_walk_boundary_mode,
        resolve_commit_walk_policy, resolve_parallel_worker_count, should_probe_max_file_size,
        should_run_rename_detection, should_use_parallel_file_change_extraction,
        validate_repository, BloomFilterView, CommitWalkBoundaryMode, CommitWalkScope,
        CommitWalkSortMode, FileChangeExtractionMode, FileChangeParallelProfile,
        BLOOM_HASH_VERSION_GIT_V1, BLOOM_MURMUR_SEED0, BLOOM_MURMUR_SEED1, COMMIT_WALK_SCOPE_ENV,
        COMMIT_WALK_SORT_ENV, FILE_CHANGE_DRAIN_CHANNEL_CAPACITY_DEFAULT,
        FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE_DEFAULT, FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS,
        FILE_CHANGE_PARSE_CHANNEL_CAPACITY, FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX,
        FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE, FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE,
        FILE_CHANGE_PARSE_WORKER_THREADS_DEFAULT_MAX, FILE_CHANGE_RENAME_HOT_PATH_ENV,
        FILE_CHANGE_RENAME_LARGE_FILE_BYTES, FILE_CHANGE_RENAME_LIMIT,
        FILE_CHANGE_RENAME_LIMIT_LARGE_DEFAULT, FILE_CHANGE_RENAME_LIMIT_XLARGE_DEFAULT,
        FILE_CHANGE_RENAME_LIMIT_XLARGE_MIN_DELTAS, FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES,
        FILE_CHANGE_RENAME_THRESHOLD_LARGE, FILE_CHANGE_RENAME_THRESHOLD_MEDIUM,
        FILE_CHANGE_RENAME_THRESHOLD_SMALL, FILE_CHANGE_SYNC_STRICT_ENV,
        GIX_OBJECT_CACHE_BYTES_DEFAULT, GIX_OBJECT_CACHE_BYTES_MAX, GIX_OBJECT_CACHE_BYTES_MIN,
        LEVEL3_TOPO_EXTRACTION_ORDER_ENV,
    };
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn unknown_repo_returns_error() {
        let result = validate_repository(Path::new("c:/non-existent/repo"));
        assert!(result.is_err());
    }

    #[test]
    fn rename_threshold_is_adaptive_by_delta_count_and_file_size() {
        assert_eq!(
            rename_threshold_for_bins(8, 1024),
            FILE_CHANGE_RENAME_THRESHOLD_SMALL
        );
        assert_eq!(
            rename_threshold_for_bins(128, 1024),
            FILE_CHANGE_RENAME_THRESHOLD_MEDIUM
        );
        assert_eq!(
            rename_threshold_for_bins(8, FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES),
            FILE_CHANGE_RENAME_THRESHOLD_MEDIUM
        );
        assert_eq!(
            rename_threshold_for_bins(8, FILE_CHANGE_RENAME_LARGE_FILE_BYTES),
            FILE_CHANGE_RENAME_THRESHOLD_LARGE
        );
    }

    #[test]
    fn rename_detection_bounds_are_configured() {
        assert!(rename_detection_policy(0, 0).is_none());

        let bounded_small = rename_detection_policy(32, 1024).expect("policy for non-zero deltas");
        assert_eq!(bounded_small.rename_limit, 32);
        assert_eq!(
            bounded_small.rename_threshold,
            FILE_CHANGE_RENAME_THRESHOLD_SMALL
        );
        assert!(!bounded_small.exact_match_only);

        let bounded_large =
            rename_detection_policy(FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS + 1, 1024)
                .expect("policy for large delta set");
        assert_eq!(
            bounded_large.rename_limit,
            FILE_CHANGE_RENAME_LIMIT_LARGE_DEFAULT
        );
        assert_eq!(
            bounded_large.rename_threshold,
            FILE_CHANGE_RENAME_THRESHOLD_LARGE
        );
        assert!(bounded_large.exact_match_only);

        let bounded_xlarge =
            rename_detection_policy(FILE_CHANGE_RENAME_LIMIT_XLARGE_MIN_DELTAS, 1024)
                .expect("policy for very large delta set");
        assert_eq!(
            bounded_xlarge.rename_limit,
            FILE_CHANGE_RENAME_LIMIT_XLARGE_DEFAULT
        );
    }

    #[test]
    fn rename_limit_is_scale_aware_for_large_delta_sets() {
        assert_eq!(rename_limit_for_bins(16), FILE_CHANGE_RENAME_LIMIT);
        assert_eq!(
            rename_limit_for_bins(FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS),
            FILE_CHANGE_RENAME_LIMIT_LARGE_DEFAULT
        );
        assert_eq!(
            rename_limit_for_bins(FILE_CHANGE_RENAME_LIMIT_XLARGE_MIN_DELTAS),
            FILE_CHANGE_RENAME_LIMIT_XLARGE_DEFAULT
        );
    }

    #[test]
    fn rename_detection_runs_only_when_diff_shape_requires_it() {
        let _guard = env_lock().lock().expect("lock env");
        let previous_sync = std::env::var(FILE_CHANGE_SYNC_STRICT_ENV).ok();
        let previous_hot = std::env::var(FILE_CHANGE_RENAME_HOT_PATH_ENV).ok();
        std::env::remove_var(FILE_CHANGE_SYNC_STRICT_ENV);
        std::env::remove_var(FILE_CHANGE_RENAME_HOT_PATH_ENV);

        assert!(!should_run_rename_detection(
            FileChangeExtractionMode::MetadataOnly,
            1,
            true
        ));
        assert!(!should_run_rename_detection(
            FileChangeExtractionMode::MetadataOnly,
            16,
            false
        ));
        assert!(!should_run_rename_detection(
            FileChangeExtractionMode::MetadataOnly,
            16,
            true
        ));

        std::env::set_var(FILE_CHANGE_RENAME_HOT_PATH_ENV, "1");
        assert!(should_run_rename_detection(
            FileChangeExtractionMode::MetadataOnly,
            16,
            true
        ));
        std::env::remove_var(FILE_CHANGE_RENAME_HOT_PATH_ENV);

        std::env::set_var(FILE_CHANGE_SYNC_STRICT_ENV, "1");
        assert!(should_run_rename_detection(
            FileChangeExtractionMode::MetadataOnly,
            16,
            true
        ));

        assert!(!should_run_rename_detection(
            FileChangeExtractionMode::SelectiveLineStats,
            16,
            false
        ));
        assert!(should_run_rename_detection(
            FileChangeExtractionMode::SelectiveLineStats,
            FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS,
            false
        ));

        if let Some(value) = previous_sync {
            std::env::set_var(FILE_CHANGE_SYNC_STRICT_ENV, value);
        } else {
            std::env::remove_var(FILE_CHANGE_SYNC_STRICT_ENV);
        }
        if let Some(value) = previous_hot {
            std::env::set_var(FILE_CHANGE_RENAME_HOT_PATH_ENV, value);
        } else {
            std::env::remove_var(FILE_CHANGE_RENAME_HOT_PATH_ENV);
        }
    }

    #[test]
    fn max_file_size_probe_is_skipped_for_large_delta_sets() {
        assert!(should_probe_max_file_size(
            FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS.saturating_sub(1)
        ));
        assert!(!should_probe_max_file_size(
            FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS
        ));
        assert!(!should_probe_max_file_size(
            FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS.saturating_add(1)
        ));
    }

    #[test]
    fn parallel_file_change_plan_is_bounded() {
        let plan_small = parallel_file_change_plan(
            FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE,
            1,
            FileChangeParallelProfile::Interactive,
        );
        assert!((FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE
            ..=FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE)
            .contains(&plan_small.chunk_size));
        assert!(
            (FILE_CHANGE_PARSE_CHANNEL_CAPACITY..=FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX)
                .contains(&plan_small.channel_capacity)
        );
        assert_eq!(plan_small.wave_chunk_limit, plan_small.channel_capacity);

        let plan_large =
            parallel_file_change_plan(10_000, 16, FileChangeParallelProfile::Interactive);
        assert!((FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE
            ..=FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE)
            .contains(&plan_large.chunk_size));
        assert!(
            (FILE_CHANGE_PARSE_CHANNEL_CAPACITY..=FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX)
                .contains(&plan_large.channel_capacity)
        );
        assert_eq!(plan_large.wave_chunk_limit, plan_large.channel_capacity);
    }

    #[test]
    fn drain_parallel_profile_uses_larger_defaults() {
        let plan = parallel_file_change_plan(10_000, 8, FileChangeParallelProfile::Drain);
        assert_eq!(
            plan.chunk_size,
            FILE_CHANGE_DRAIN_PARALLEL_CHUNK_SIZE_DEFAULT
        );
        assert_eq!(
            plan.channel_capacity,
            FILE_CHANGE_DRAIN_CHANNEL_CAPACITY_DEFAULT
        );
        assert_eq!(plan.wave_chunk_limit, plan.channel_capacity);
    }

    #[test]
    fn parallel_activation_supports_incremental_batches() {
        let threshold = parallel_min_commits_threshold_for_workers(4, None);
        assert!(
            threshold <= 10,
            "expected adaptive threshold to allow 10-commit batches"
        );
        assert!(should_use_parallel_file_change_extraction(10, 4, threshold));
        assert!(!should_use_parallel_file_change_extraction(
            threshold.saturating_sub(1),
            4,
            threshold
        ));
        assert!(!should_use_parallel_file_change_extraction(40, 1, 1));
    }

    #[test]
    fn parallel_worker_count_respects_bounds_and_overrides() {
        assert_eq!(
            resolve_parallel_worker_count(16, None),
            FILE_CHANGE_PARSE_WORKER_THREADS_DEFAULT_MAX
        );
        assert_eq!(resolve_parallel_worker_count(4, Some(2)), 2);
        assert_eq!(resolve_parallel_worker_count(4, Some(10)), 4);
        assert_eq!(resolve_parallel_worker_count(0, None), 1);
    }

    #[test]
    fn object_cache_size_scales_and_stays_bounded() {
        assert_eq!(
            object_cache_bytes_for_workers(0),
            GIX_OBJECT_CACHE_BYTES_DEFAULT
        );
        assert_eq!(
            object_cache_bytes_for_workers(1),
            GIX_OBJECT_CACHE_BYTES_DEFAULT
        );
        assert_eq!(object_cache_bytes_for_workers(2), 32 * 1024 * 1024);
        assert_eq!(
            object_cache_bytes_for_workers(4),
            GIX_OBJECT_CACHE_BYTES_MIN
        );
        assert_eq!(
            object_cache_bytes_for_workers(32),
            GIX_OBJECT_CACHE_BYTES_MIN
        );
        assert!(object_cache_bytes_for_workers(1) <= GIX_OBJECT_CACHE_BYTES_MAX);
    }

    #[test]
    fn parse_hex_object_id_validates_input() {
        assert!(parse_hex_object_id(&"a".repeat(40), "parse commit sha").is_ok());
        assert!(parse_hex_object_id("not-hex", "parse commit sha").is_err());
        assert!(parse_hex_object_id(&"a".repeat(39), "parse commit sha").is_err());
        assert!(parse_hex_object_id(&"a".repeat(42), "parse commit sha").is_err());
        assert!(parse_hex_object_id(&"a".repeat(66), "parse commit sha").is_err());
    }

    #[test]
    fn level3_runtime_flags_follow_env_toggles() {
        let _guard = env_lock().lock().expect("lock env");
        let previous_topo = std::env::var(LEVEL3_TOPO_EXTRACTION_ORDER_ENV).ok();
        std::env::remove_var(LEVEL3_TOPO_EXTRACTION_ORDER_ENV);

        let defaults = level3_runtime_options();
        assert!(defaults.topology_aware_extraction);

        std::env::set_var(LEVEL3_TOPO_EXTRACTION_ORDER_ENV, "on");

        let options = level3_runtime_options();
        assert!(options.topology_aware_extraction);

        if let Some(value) = previous_topo {
            std::env::set_var(LEVEL3_TOPO_EXTRACTION_ORDER_ENV, value);
        } else {
            std::env::remove_var(LEVEL3_TOPO_EXTRACTION_ORDER_ENV);
        }
    }

    #[test]
    fn commit_walk_scope_defaults_to_frontier_and_supports_all_refs_override() {
        let _guard = env_lock().lock().expect("lock env");
        let previous_scope = std::env::var(COMMIT_WALK_SCOPE_ENV).ok();
        std::env::remove_var(COMMIT_WALK_SCOPE_ENV);
        assert_eq!(parse_commit_walk_scope(), CommitWalkScope::Frontier);

        std::env::set_var(COMMIT_WALK_SCOPE_ENV, "all_refs");
        assert_eq!(parse_commit_walk_scope(), CommitWalkScope::AllRefs);

        std::env::set_var(COMMIT_WALK_SCOPE_ENV, "unknown");
        assert_eq!(parse_commit_walk_scope(), CommitWalkScope::Frontier);

        if let Some(value) = previous_scope {
            std::env::set_var(COMMIT_WALK_SCOPE_ENV, value);
        } else {
            std::env::remove_var(COMMIT_WALK_SCOPE_ENV);
        }
    }

    #[test]
    fn commit_walk_sort_policy_defaults_to_adaptive_auto_without_flags() {
        let _guard = env_lock().lock().expect("lock env");
        let previous_sort = std::env::var(COMMIT_WALK_SORT_ENV).ok();

        std::env::remove_var(COMMIT_WALK_SORT_ENV);
        assert_eq!(
            resolve_commit_walk_policy(None, None).sort_mode,
            CommitWalkSortMode::CommitTimeNewest
        );
        assert_eq!(
            resolve_commit_walk_policy(Some(123), None).sort_mode,
            CommitWalkSortMode::CommitTimeNewest
        );
        assert_eq!(
            resolve_commit_walk_policy(None, Some("abc")).sort_mode,
            CommitWalkSortMode::CommitTimeNewest
        );

        std::env::set_var(COMMIT_WALK_SORT_ENV, "commit_time_cutoff");
        assert_eq!(
            resolve_commit_walk_policy(None, None).sort_mode,
            CommitWalkSortMode::CommitTimeCutoff
        );

        if let Some(value) = previous_sort {
            std::env::set_var(COMMIT_WALK_SORT_ENV, value);
        } else {
            std::env::remove_var(COMMIT_WALK_SORT_ENV);
        }
    }

    #[test]
    fn commit_walk_boundary_mode_is_checkpoint_stop_when_hide_is_present() {
        assert_eq!(
            resolve_commit_walk_boundary_mode(None),
            CommitWalkBoundaryMode::NotApplicable
        );
        assert_eq!(
            resolve_commit_walk_boundary_mode(Some("abc")),
            CommitWalkBoundaryMode::CheckpointStop
        );
    }

    #[test]
    fn topology_reorder_with_parent_map_orders_ancestors_first() {
        let commits = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        let parents_by_sha = HashMap::from([
            ("a".to_string(), Vec::<String>::new()),
            ("b".to_string(), vec!["a".to_string()]),
            ("c".to_string(), vec!["b".to_string()]),
        ]);

        let ordered = reorder_commits_topologically_with_parents(&commits, &parents_by_sha);
        assert_eq!(
            ordered,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn normalize_repo_relative_path_handles_windows_and_dot_prefixes() {
        assert_eq!(
            normalize_repo_relative_path(".\\src\\adapters\\git.rs"),
            "src/adapters/git.rs"
        );
        assert_eq!(
            normalize_repo_relative_path("./src/application/indexer.rs"),
            "src/application/indexer.rs"
        );
        assert_eq!(normalize_repo_relative_path("/src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn bloom_membership_check_matches_seeded_bits() {
        let key = b"src/adapters/git.rs";
        let mut bytes = vec![0u8; 16];
        let total_bits = bytes.len() * 8;
        let num_hashes = 7u32;

        let hash0 = bloom_murmur3_seeded(BLOOM_HASH_VERSION_GIT_V1, BLOOM_MURMUR_SEED0, key);
        let hash1 = bloom_murmur3_seeded(BLOOM_HASH_VERSION_GIT_V1, BLOOM_MURMUR_SEED1, key);
        for round in 0..usize::try_from(num_hashes).unwrap_or(0) {
            let hash = hash0.wrapping_add((round as u32).wrapping_mul(hash1));
            let bit_pos = usize::try_from(hash).unwrap_or(0) % total_bits;
            bytes[bit_pos / 8] |= 1u8 << (bit_pos & 7);
        }

        let filter = BloomFilterView {
            bytes: bytes.as_slice(),
            hash_version: BLOOM_HASH_VERSION_GIT_V1,
            num_hashes,
        };
        assert!(bloom_filter_contains_key(filter, key));
        assert!(!bloom_filter_contains_key(filter, b"src/main.rs"));
    }

    #[test]
    fn zeroed_bloom_filter_implies_no_first_parent_changes() {
        let no_change = BloomFilterView {
            bytes: &[0u8],
            hash_version: BLOOM_HASH_VERSION_GIT_V1,
            num_hashes: 7,
        };
        assert!(bloom_filter_indicates_no_first_parent_changes(no_change));

        let has_change = BloomFilterView {
            bytes: &[0xff],
            hash_version: BLOOM_HASH_VERSION_GIT_V1,
            num_hashes: 7,
        };
        assert!(!bloom_filter_indicates_no_first_parent_changes(has_change));
    }

    #[test]
    fn bloom_filter_hook_fails_open_for_unresolved_commits() {
        let commit_shas = vec!["a".repeat(40), "b".repeat(40)];
        let mut targets = HashMap::<String, HashSet<String>>::new();
        targets.insert(
            commit_shas[0].clone(),
            HashSet::from(["src/lib.rs".to_string()]),
        );
        targets.insert(
            commit_shas[1].clone(),
            HashSet::from(["src/main.rs".to_string()]),
        );

        let filtered = filter_commits_for_line_stat_targets(Path::new("."), &commit_shas, &targets)
            .expect("filter should fail-open and preserve unresolved commits");
        assert_eq!(filtered, commit_shas);
    }
}
