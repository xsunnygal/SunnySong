import { invoke } from "@tauri-apps/api/core";

export interface AppStatus {
	name: string;
	version: string;
	ready: boolean;
}
export interface YouTubeAuthStatus {
	configured: boolean;
}
export interface ListeningProfile {
	id: string;
	name: string;
	createdAtMs: number;
	lastUsedAtMs: number;
}
export interface Playlist {
	id: string;
	name: string;
	createdAtMs: number;
	updatedAtMs: number;
	trackCount: number;
}
export interface PlaylistTrack {
	song: Song;
	position: number;
	addedAtMs: number;
}
export interface Song {
	id: string;
	title: string;
	artistId: string | null;
	artistName: string;
	albumId: string | null;
	albumName: string | null;
	durationMs: number | null;
	thumbnailUrl: string | null;
}
export type AudioQuality = "low" | "medium" | "high";
export type CatalogFilter =
	"all" | "songs" | "channels" | "playlists" | "albums";
export interface CatalogArtist {
	id: string;
	name: string;
	thumbnailUrl: string | null;
	subtitle: string | null;
	source: "youtube" | "local" | string;
}
export interface CatalogCollection {
	id: string;
	title: string;
	subtitle: string | null;
	thumbnailUrl: string | null;
	kind: "album" | "playlist" | "single" | "ep" | "release" | string;
}
export interface CatalogSearchResults {
	songs: Song[];
	artists: CatalogArtist[];
	albums: CatalogCollection[];
	playlists: CatalogCollection[];
}
export interface ArtistPage {
	artist: CatalogArtist;
	topSongs: Song[];
	songs: Song[];
	latestReleases: CatalogCollection[];
	albums: CatalogCollection[];
	singles: CatalogCollection[];
	playlists: CatalogCollection[];
}
export interface AndroidStorageDirectory {
	uri: string;
	displayName: string;
	canWrite: boolean;
	persisted: boolean;
}
export interface DownloadResult {
	location: string;
	fileName: string;
}
export interface SongLyrics {
	text: string;
	source: "embedded" | "youtube";
	attribution: string | null;
}
export interface PlaybackSource {
	url: string;
	mimeType: string;
	expiresAtMs: number | null;
}
export interface PlaybackState {
	current: Song | null;
	queue: Song[];
	currentIndex: number | null;
}
export interface QuickPickOptions {
	diverse: boolean;
	newSongs: boolean;
	rediscover: boolean;
}
export interface PlaybackPreparation {
	source: PlaybackSource;
	state: PlaybackState;
}
export interface MediaSessionUpdate {
	active: boolean;
	title: string;
	artist: string;
	album: string | null;
	artworkUrl: string | null;
	playing: boolean;
	positionMs: number;
	durationMs: number;
	canGoPrevious: boolean;
	canGoNext: boolean;
}
export interface PlaybackSummary {
	eventId: string;
	profileId: string;
	song: Song;
	startedAtMs: number;
	listenedMs: number;
	durationMs: number | null;
	reason: "completed" | "next" | "previous" | "replaced" | "stopped" | "failed";
}
export interface ScoreComponent {
	name: string;
	rawValue: number;
	contribution: number;
}
export interface Recommendation {
	song: Song;
	score: number;
	reasons: string[];
	source: string;
	policyVersion: string;
	components: ScoreComponent[];
}
export interface DatabaseDiagnostics {
	schemaVersion: number;
	databaseSizeBytes: number;
	trackCount: number;
	artistCount: number;
	historyEventCount: number;
	likedSongCount: number;
	integrityStatus: string;
	queryDurationMs: number;
}
export interface DeveloperDiagnostics {
	generatedAtMs: number;
	activeProfile: ListeningProfile;
	database: DatabaseDiagnostics;
	player: PlaybackState;
	recommendations: Recommendation[];
}
export interface MusicDirectory {
	id: number;
	path: string;
	addedAtMs: number;
	lastScannedAtMs: number | null;
	lastScanAttemptAtMs: number | null;
	status: "READY" | "SCANNING" | "UNAVAILABLE" | "ERROR";
	lastError: string | null;
	trackCount: number;
}
export interface JellyfinLibrary {
	id: string;
	name: string;
	collectionType: string | null;
	enabled: boolean;
	lastSyncAtMs: number | null;
	trackCount: number;
}
export interface JellyfinServer {
	id: number;
	serverInternalId: string;
	name: string;
	baseUrl: string;
	username: string;
	status: string;
	lastConnectedAtMs: number | null;
	lastSyncAtMs: number | null;
	lastError: string | null;
	libraries: JellyfinLibrary[];
}
export interface JellyfinRefreshResult {
	server: JellyfinServer;
	diagnostics: string[];
}
export interface JellyfinPublicServerInfo {
	name: string;
	serverId: string;
	version: string;
	normalizedUrl: string;
}
export interface JellyfinQuickConnectSession {
	code: string;
	secret: string;
}
export interface LocalArtist {
	id: number;
	name: string;
	enabled: boolean;
	trackCount: number;
}
export type LibrarySource = "local" | "jellyfin";
export interface LibraryTrack {
	song: Song;
	source: LibrarySource;
	sourceName: string;
}
export interface LibraryAlbum {
	id: string;
	title: string;
	artistName: string;
	thumbnailUrl: string | null;
	trackCount: number;
	source: LibrarySource;
	sourceName: string;
}
export interface LibraryFolder {
	id: string;
	name: string;
	detail: string;
	trackCount: number;
	source: LibrarySource;
}
export interface LibraryScanResult {
	directoryId: number;
	status: string;
	indexedTracks: number;
	unavailableTracks: number;
	skippedFiles: number;
	durationMs: number;
	error: string | null;
}
export interface RecentSongsPage {
	items: Song[];
	nextCursor: number | null;
	hasMore: boolean;
}

export const getAppStatus = () => invoke<AppStatus>("get_app_status");

export const searchMusic = (query: string) =>
	invoke<Song[]>("search_music", { query });
export const getPlaylists = () => invoke<Playlist[]>("get_playlists");
export const createPlaylist = (name: string) =>
	invoke<Playlist>("create_playlist", { name });
export const getPlaylistTracks = (playlistId: string) =>
	invoke<PlaylistTrack[]>("get_playlist_tracks", { playlistId });
export const addSongToPlaylist = (playlistId: string, song: Song) =>
	invoke<PlaylistTrack>("add_song_to_playlist", { playlistId, song });
export const searchLocalMusic = (query: string) =>
	invoke<Song[]>("search_local_music", { query });
export const getLibraryTracks = (count = 12, offset = 0) =>
	invoke<LibraryTrack[]>("get_library_tracks", { count, offset });
export const getLibraryAlbums = (count = 8, offset = 0) =>
	invoke<LibraryAlbum[]>("get_library_albums", { count, offset });
export const getLibraryAlbumTracks = (albumId: string) =>
	invoke<LibraryTrack[]>("get_library_album_tracks", { albumId });
export const getLibraryFolders = () =>
	invoke<LibraryFolder[]>("get_library_folders");
export const searchCatalog = (query: string, filter: CatalogFilter = "all") =>
	invoke<CatalogSearchResults>("search_catalog", { query, filter });
export const getArtistPage = (artistId: string) =>
	invoke<ArtistPage>("get_artist_page", { artistId });
export const getCollectionSongs = (collectionId: string) =>
	invoke<Song[]>("get_collection_songs", { collectionId });
export const getDiscoveryEnabled = () =>
	invoke<boolean>("get_discovery_enabled");
export const setAudioQuality = (quality: AudioQuality) =>
	invoke<void>("set_audio_quality", { quality });
export const setDiscoveryEnabled = (enabled: boolean) =>
	invoke<void>("set_discovery_enabled", { enabled });
export const getYouTubeAuthStatus = () =>
	invoke<YouTubeAuthStatus>("get_youtube_auth_status");
export const setYouTubeCookies = (cookies: string) =>
	invoke<YouTubeAuthStatus>("set_youtube_cookies", { cookies });
export const clearYouTubeAuth = () =>
	invoke<YouTubeAuthStatus>("clear_youtube_auth");
export const validateJellyfinServer = (address: string) =>
	invoke<JellyfinPublicServerInfo>("validate_jellyfin_server", { address });
export const connectJellyfinPassword = (
	address: string,
	username: string,
	password: string,
) =>
	invoke<JellyfinServer>("connect_jellyfin_password", {
		address,
		username,
		password,
	});
export const beginJellyfinQuickConnect = (address: string) =>
	invoke<JellyfinQuickConnectSession>("begin_jellyfin_quick_connect", {
		address,
	});
export const finishJellyfinQuickConnect = (address: string, secret: string) =>
	invoke<JellyfinServer | null>("finish_jellyfin_quick_connect", {
		address,
		secret,
	});
export const getJellyfinServers = () =>
	invoke<JellyfinServer[]>("get_jellyfin_servers");
export const refreshJellyfinLibraries = (serverId: number) =>
	invoke<JellyfinRefreshResult>("refresh_jellyfin_libraries", { serverId });
export const setJellyfinLibraryEnabled = (
	serverId: number,
	libraryId: string,
	enabled: boolean,
) =>
	invoke<void>("set_jellyfin_library_enabled", {
		serverId,
		libraryId,
		enabled,
	});
export const removeJellyfinServer = (serverId: number) =>
	invoke<void>("remove_jellyfin_server", { serverId });
export const getMusicDirectories = () =>
	invoke<MusicDirectory[]>("get_music_directories");
export const addMusicDirectory = (path: string) =>
	invoke<MusicDirectory>("add_music_directory", { path });
export const removeMusicDirectory = (directoryId: number) =>
	invoke<void>("remove_music_directory", { directoryId });
export const rescanMusicLibrary = () =>
	invoke<LibraryScanResult[]>("rescan_music_library");
export const getLocalArtists = (query = "", count = 50, offset = 0) =>
	invoke<LocalArtist[]>("get_local_artists", { query, count, offset });
export const setLocalArtistEnabled = (artistId: number, enabled: boolean) =>
	invoke<void>("set_local_artist_enabled", { artistId, enabled });
export const downloadSong = (
	song: Song,
	directoryId: number | null,
	androidTreeUri: string | null,
) =>
	invoke<DownloadResult>("download_song", {
		song,
		directoryId,
		androidTreeUri,
	});
export const pickAndroidDownloadDirectory = () =>
	invoke<AndroidStorageDirectory>("pick_android_download_directory");
export const backgroundApp = () => invoke<void>("background_app");
export const getSongLyrics = (songId: string) =>
	invoke<SongLyrics | null>("get_song_lyrics", { songId });
export const preparePlayback = (song: Song) =>
	invoke<PlaybackPreparation>("prepare_playback", { song });
export const warmPlaybackSource = (songId: string) =>
	invoke<void>("warm_playback_source", { songId });
export const hydratePlaybackQueue = (songId: string) =>
	invoke<PlaybackState>("hydrate_playback_queue", { songId });
export const refillPlaybackQueue = (count: number) =>
	invoke<PlaybackState>("refill_playback_queue", { count });
export const playNext = () => invoke<PlaybackPreparation>("play_next");
export const playQueueItem = (songId: string) =>
	invoke<PlaybackPreparation>("play_queue_item", { songId });
export const playPrevious = () => invoke<PlaybackPreparation>("play_previous");
export const refreshPlaybackSource = () =>
	invoke<PlaybackPreparation>("refresh_playback_source");
export const getPlaybackState = () =>
	invoke<PlaybackState>("get_playback_state");
export const getListeningProfiles = () =>
	invoke<ListeningProfile[]>("get_listening_profiles");
export const getActiveListeningProfile = () =>
	invoke<ListeningProfile>("get_active_listening_profile");
export const createListeningProfile = (name: string) =>
	invoke<ListeningProfile>("create_listening_profile", { name });
export const renameListeningProfile = (profileId: string, name: string) =>
	invoke<ListeningProfile>("rename_listening_profile", { profileId, name });
export const deleteListeningProfile = (profileId: string) =>
	invoke<void>("delete_listening_profile", { profileId });
export const setActiveListeningProfile = (profileId: string) =>
	invoke<ListeningProfile>("set_active_listening_profile", { profileId });
export const getLikedSongIds = () => invoke<string[]>("get_liked_song_ids");
export const reportPlayback = (summary: PlaybackSummary) =>
	invoke<void>("report_playback", { summary });
export const syncMediaSession = (update: MediaSessionUpdate) =>
	invoke<void>("sync_media_session", { update });
export const setReaction = (song: Song, liked: boolean, disliked: boolean) =>
	invoke<void>("set_reaction", { song, liked, disliked });
export const getRecentSongs = (count = 10, before: number | null = null) =>
	invoke<RecentSongsPage>("get_recent_songs", { count, before });
export const getQuickPicks = (
	count = 8,
	exclude: string[] = [],
	options: QuickPickOptions = {
		diverse: true,
		newSongs: true,
		rediscover: true,
	},
) => invoke<Recommendation[]>("get_quick_picks", { count, exclude, options });
export const getDeveloperDiagnostics = () =>
	invoke<DeveloperDiagnostics>("get_developer_diagnostics");
