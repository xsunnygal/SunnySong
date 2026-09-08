package app.solmusic.storage

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.os.Bundle

import android.os.SystemClock
import android.support.v4.media.MediaBrowserCompat
import android.support.v4.media.MediaDescriptionCompat
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.media.MediaBrowserServiceCompat
import androidx.media.MediaSessionManager
import androidx.media.app.NotificationCompat.MediaStyle
import java.net.URL
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.Executors

class PlaybackService : MediaBrowserServiceCompat() {
    private data class PlaybackInfo(
        val title: String,
        val artist: String,
        val album: String?,
        val artworkUrl: String?,
        val playing: Boolean,
        val positionMs: Long,
        val durationMs: Long,
        val canGoPrevious: Boolean,
        val canGoNext: Boolean,
    )

    private lateinit var mediaSession: MediaSessionCompat
    private val artworkExecutor = Executors.newSingleThreadExecutor()
    private var browseSnapshot = BrowseSnapshotStore.Snapshot(emptyList(), null, null)
    private var playback: PlaybackInfo? = null
    private var foregroundActive = false
    private var artwork: Bitmap? = null
    private var loadedArtworkUrl: String? = null

    override fun onCreate() {
        super.onCreate()
        isRunning = true
        createNotificationChannel()
        browseSnapshot = BrowseSnapshotStore.load(this)
        playback = browseSnapshot.current?.let {
            PlaybackInfo(
                title = it.title,
                artist = it.artist,
                album = it.album,
                artworkUrl = it.artworkUrl,
                playing = false,
                positionMs = 0,
                durationMs = it.durationMs,
                canGoPrevious = (browseSnapshot.currentIndex ?: 0) > 0,
                canGoNext = (browseSnapshot.currentIndex ?: -1) + 1 < browseSnapshot.queue.size,
            )
        }
        mediaSession = MediaSessionCompat(this, "SunnySongPlayback").apply {
            setCallback(object : MediaSessionCompat.Callback() {
                override fun onPlay() = dispatch(CONTROL_PLAY)
                override fun onPause() = dispatch(CONTROL_PAUSE)
                override fun onSkipToNext() = dispatch(CONTROL_NEXT)
                override fun onSkipToPrevious() = dispatch(CONTROL_PREVIOUS)
                override fun onSeekTo(position: Long) = dispatch(CONTROL_SEEK, position)
                override fun onStop() {
                    dispatch(CONTROL_STOP)
                    stopPlaybackService()
                }

                override fun onPlayFromMediaId(mediaId: String?, extras: Bundle?) {
                    playFromMediaId(mediaId)
                }

                override fun onSkipToQueueItem(id: Long) {
                    browseSnapshot.queue.firstOrNull { queueId(it.mediaId) == id }?.let(::dispatchPlayable)
                }
            })
            setSessionActivity(openAppIntent())
        }
        sessionToken = mediaSession.sessionToken
        updateMediaSession()
        loadArtwork(playback?.artworkUrl)
    }

    override fun onGetRoot(
        clientPackageName: String,
        clientUid: Int,
        rootHints: Bundle?,
    ): BrowserRoot? {
        val packages = packageManager.getPackagesForUid(clientUid)?.toSet().orEmpty()
        val packageMatchesUid = clientPackageName.isNotBlank() && clientPackageName in packages
        val trustedController = packageMatchesUid &&
            MediaSessionManager.getSessionManager(this).isTrustedForMediaControl(
                MediaSessionManager.RemoteUserInfo(clientPackageName, -1, clientUid)
            )
        if (clientUid != applicationInfo.uid && !trustedController) return null
        val extras = Bundle().apply {
            putInt("android.media.browse.CONTENT_STYLE_BROWSABLE_HINT", 1)
            putInt("android.media.browse.CONTENT_STYLE_PLAYABLE_HINT", 1)
        }
        return BrowserRoot(ROOT_ID, extras)
    }

    override fun onLoadChildren(parentId: String, result: Result<List<MediaBrowserCompat.MediaItem>>) {
        val children = when (parentId) {
            ROOT_ID -> buildList {
                add(browsableItem(QUEUE_ID, getString(R.string.auto_queue)))
                if (browseSnapshot.current != null) {
                    add(browsableItem(RESUME_ID, getString(R.string.auto_resume)))
                }
            }
            QUEUE_ID -> browseSnapshot.queue.map(::playableItem)
            RESUME_ID -> listOfNotNull(browseSnapshot.current?.let(::playableItem))
            else -> emptyList()
        }
        result.sendResult(children)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.let { it.action in INTERNAL_ACTIONS && !hasValidCommandToken(it) } == true) {
            return if (foregroundActive) START_STICKY else START_NOT_STICKY
        }
        when (intent?.action) {
            ACTION_UPDATE -> updateFrom(intent)
            ACTION_PLAY -> dispatch(CONTROL_PLAY)
            ACTION_PAUSE -> dispatch(CONTROL_PAUSE)
            ACTION_NEXT -> dispatch(CONTROL_NEXT)
            ACTION_PREVIOUS -> dispatch(CONTROL_PREVIOUS)
            ACTION_PLAY_FROM_MEDIA_ID -> playFromMediaId(intent.getStringExtra(EXTRA_MEDIA_ID))
            ACTION_DISMISS -> {
                dispatch(CONTROL_STOP)
                stopPlaybackService()
                return START_NOT_STICKY
            }
        }
        return if (foregroundActive) START_STICKY else START_NOT_STICKY
    }

    private fun updateFrom(intent: Intent) {
        browseSnapshot = BrowseSnapshotStore.load(this)
        notifyChildrenChanged(ROOT_ID)
        notifyChildrenChanged(QUEUE_ID)
        notifyChildrenChanged(RESUME_ID)
        if (!intent.getBooleanExtra(EXTRA_ACTIVE, false)) {
            stopPlaybackService()
            return
        }
        playback = PlaybackInfo(
            title = intent.getStringExtra(EXTRA_TITLE).orEmpty(),
            artist = intent.getStringExtra(EXTRA_ARTIST).orEmpty(),
            album = intent.getStringExtra(EXTRA_ALBUM),
            artworkUrl = sanitizeArtworkUrl(intent.getStringExtra(EXTRA_ARTWORK_URL)),
            playing = intent.getBooleanExtra(EXTRA_PLAYING, false),
            positionMs = intent.getLongExtra(EXTRA_POSITION_MS, 0L).coerceAtLeast(0L),
            durationMs = intent.getLongExtra(EXTRA_DURATION_MS, 0L).coerceAtLeast(0L),
            canGoPrevious = intent.getBooleanExtra(EXTRA_CAN_GO_PREVIOUS, false),
            canGoNext = intent.getBooleanExtra(EXTRA_CAN_GO_NEXT, false),
        )
        updateMediaSession()
        foregroundActive = true
        startForeground(NOTIFICATION_ID, buildNotification())
        loadArtwork(playback?.artworkUrl)
    }

    private fun playFromMediaId(mediaId: String?) {
        if (mediaId.isNullOrBlank()) return
        val item = browseSnapshot.current?.takeIf { it.mediaId == mediaId }
            ?: browseSnapshot.queue.firstOrNull { it.mediaId == mediaId }
        item?.let(::dispatchPlayable)
    }

    private fun dispatchPlayable(item: BrowseSnapshotStore.Item) {
        val queueIndex = browseSnapshot.queue.indexOfFirst { it.mediaId == item.mediaId }
            .takeIf { it >= 0 }
        StoragePlugin.dispatchMediaControl(
            action = CONTROL_PLAY_FROM_MEDIA_ID,
            mediaId = item.mediaId,
            songId = item.sourceId,
            queueIndex = queueIndex,
        )
    }

    private fun dispatch(action: String, positionMs: Long? = null) {
        if (playback == null && browseSnapshot.current == null) return
        StoragePlugin.dispatchMediaControl(action, positionMs = positionMs)
        // HTMLAudioElement owns decoding and authoritative state. The frontend's next
        // snapshot updates playback state and the notification after the action succeeds.
    }

    private fun updateMediaSession() {
        val current = playback
        if (current == null) {
            mediaSession.setPlaybackState(
                PlaybackStateCompat.Builder()
                    .setActions(PlaybackStateCompat.ACTION_PLAY_FROM_MEDIA_ID)
                    .setState(PlaybackStateCompat.STATE_NONE, 0, 0f)
                    .build()
            )
            mediaSession.setMetadata(null)
            mediaSession.setQueue(null)
            mediaSession.isActive = browseSnapshot.current != null
            return
        }
        var actions = PlaybackStateCompat.ACTION_PLAY or
            PlaybackStateCompat.ACTION_PAUSE or
            PlaybackStateCompat.ACTION_PLAY_PAUSE or
            PlaybackStateCompat.ACTION_PLAY_FROM_MEDIA_ID or
            PlaybackStateCompat.ACTION_SEEK_TO or
            PlaybackStateCompat.ACTION_STOP
        if (current.canGoPrevious) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS
        if (current.canGoNext) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_NEXT
        if (browseSnapshot.queue.isNotEmpty()) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_QUEUE_ITEM
        mediaSession.setPlaybackState(
            PlaybackStateCompat.Builder()
                .setActions(actions)
                .setActiveQueueItemId(activeQueueId())
                .setState(
                    if (current.playing) PlaybackStateCompat.STATE_PLAYING else PlaybackStateCompat.STATE_PAUSED,
                    current.positionMs,
                    if (current.playing) 1f else 0f,
                    SystemClock.elapsedRealtime(),
                )
                .build()
        )
        val metadata = MediaMetadataCompat.Builder()
            .putString(MediaMetadataCompat.METADATA_KEY_TITLE, current.title)
            .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, current.artist)
            .putString(MediaMetadataCompat.METADATA_KEY_ALBUM, current.album.orEmpty())
            .putLong(MediaMetadataCompat.METADATA_KEY_DURATION, current.durationMs)
        artwork?.let {
            metadata.putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, it)
            metadata.putBitmap(MediaMetadataCompat.METADATA_KEY_DISPLAY_ICON, it)
        }
        mediaSession.setMetadata(metadata.build())
        mediaSession.setQueue(
            browseSnapshot.queue.map { item ->
                MediaSessionCompat.QueueItem(description(item), queueId(item.mediaId))
            }
        )
        mediaSession.isActive = true
    }

    private fun browsableItem(mediaId: String, title: String) = MediaBrowserCompat.MediaItem(
        MediaDescriptionCompat.Builder().setMediaId(mediaId).setTitle(title).build(),
        MediaBrowserCompat.MediaItem.FLAG_BROWSABLE,
    )

    private fun playableItem(item: BrowseSnapshotStore.Item) = MediaBrowserCompat.MediaItem(
        description(item),
        MediaBrowserCompat.MediaItem.FLAG_PLAYABLE,
    )

    private fun description(item: BrowseSnapshotStore.Item): MediaDescriptionCompat {
        val extras = Bundle().apply {
            putLong(MediaMetadataCompat.METADATA_KEY_DURATION, item.durationMs)
        }
        return MediaDescriptionCompat.Builder()
            .setMediaId(item.mediaId)
            .setTitle(item.title)
            .setSubtitle(item.artist)
            .setDescription(item.album)
            .setIconUri(sanitizeArtworkUrl(item.artworkUrl)?.let(Uri::parse))
            .setExtras(extras)
            .build()
    }

    private fun activeQueueId(): Long {
        val current = browseSnapshot.current ?: return MediaSessionCompat.QueueItem.UNKNOWN_ID.toLong()
        return queueId(current.mediaId)
    }

    private fun queueId(mediaId: String): Long {
        val bytes = MessageDigest.getInstance("SHA-256").digest(mediaId.toByteArray(Charsets.UTF_8))
        var value = 0L
        for (index in 0 until Long.SIZE_BYTES) value = (value shl 8) or (bytes[index].toLong() and 0xff)
        return value and Long.MAX_VALUE
    }

    private fun buildNotification(): android.app.Notification {
        val current = requireNotNull(playback)
        val previous = servicePendingIntent(ACTION_PREVIOUS, 1)
        val toggleAction = if (current.playing) ACTION_PAUSE else ACTION_PLAY
        val toggleIcon = if (current.playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play
        val toggleLabel = getString(if (current.playing) R.string.playback_pause else R.string.playback_play)
        val toggle = servicePendingIntent(toggleAction, 2)
        val next = servicePendingIntent(ACTION_NEXT, 3)
        val dismiss = servicePendingIntent(ACTION_DISMISS, 4)

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_notification_music)
            .setLargeIcon(artwork)
            .setContentTitle(current.title)
            .setContentText(current.artist)
            .setSubText(current.album)
            .setContentIntent(openAppIntent())
            .setDeleteIntent(dismiss)
            .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setOngoing(current.playing)
            .addAction(android.R.drawable.ic_media_previous, getString(R.string.playback_previous), previous)
            .addAction(toggleIcon, toggleLabel, toggle)
            .addAction(android.R.drawable.ic_media_next, getString(R.string.playback_next), next)
            .setStyle(
                MediaStyle()
                    .setMediaSession(mediaSession.sessionToken)
                    .setShowActionsInCompactView(0, 1, 2)
                    .setShowCancelButton(false)
            )
            .build()
    }

    private fun loadArtwork(value: String?) {
        val url = sanitizeArtworkUrl(value) ?: return
        if (url == loadedArtworkUrl) return
        loadedArtworkUrl = url
        artworkExecutor.execute {
            val bitmap = try {
                val connection = URL(url).openConnection().apply {
                    connectTimeout = 8_000
                    readTimeout = 8_000
                }
                connection.getInputStream().use(BitmapFactory::decodeStream)
            } catch (_: Exception) {
                null
            }
            if (loadedArtworkUrl != url || bitmap == null) return@execute
            artwork = bitmap
            updateMediaSession()
            if (foregroundActive && playback != null) {
                NotificationManagerCompat.from(this).notify(NOTIFICATION_ID, buildNotification())
            }
        }
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.playback_channel_name),
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = getString(R.string.playback_channel_description)
            setShowBadge(false)
            setSound(null, null)
        }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun servicePendingIntent(action: String, requestCode: Int): PendingIntent =
        PendingIntent.getService(
            this,
            requestCode,
            authorizedIntent(this, action),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    private fun openAppIntent(): PendingIntent {
        val intent = packageManager.getLaunchIntentForPackage(packageName)?.apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        } ?: Intent()
        return PendingIntent.getActivity(
            this,
            0,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun stopPlaybackService() {
        foregroundActive = false
        playback = null
        updateMediaSession()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        NotificationManagerCompat.from(this).cancel(NOTIFICATION_ID)
        stopSelf()
    }

    override fun onDestroy() {
        isRunning = false
        artworkExecutor.shutdownNow()
        mediaSession.release()
        super.onDestroy()
    }

    companion object {
        const val ACTION_UPDATE = "app.solmusic.playback.UPDATE"
        const val ACTION_PLAY = "app.solmusic.playback.PLAY"
        const val ACTION_PAUSE = "app.solmusic.playback.PAUSE"
        const val ACTION_NEXT = "app.solmusic.playback.NEXT"
        const val ACTION_PREVIOUS = "app.solmusic.playback.PREVIOUS"
        const val ACTION_PLAY_FROM_MEDIA_ID = "app.solmusic.playback.PLAY_FROM_MEDIA_ID"
        const val ACTION_DISMISS = "app.solmusic.playback.DISMISS"

        const val EXTRA_ACTIVE = "active"
        const val EXTRA_TITLE = "title"
        const val EXTRA_ARTIST = "artist"
        const val EXTRA_ALBUM = "album"
        const val EXTRA_ARTWORK_URL = "artworkUrl"
        const val EXTRA_PLAYING = "playing"
        const val EXTRA_POSITION_MS = "positionMs"
        const val EXTRA_DURATION_MS = "durationMs"
        const val EXTRA_CAN_GO_PREVIOUS = "canGoPrevious"
        const val EXTRA_CAN_GO_NEXT = "canGoNext"
        const val EXTRA_MEDIA_ID = "mediaId"
        private const val EXTRA_COMMAND_TOKEN = "commandToken"

        const val CONTROL_PLAY = "play"
        const val CONTROL_PAUSE = "pause"
        const val CONTROL_NEXT = "next"
        const val CONTROL_PREVIOUS = "previous"
        const val CONTROL_SEEK = "seek"
        const val CONTROL_STOP = "stop"
        const val CONTROL_PLAY_FROM_MEDIA_ID = "play-from-media-id"

        @Volatile
        var isRunning: Boolean = false
            private set

        private const val ROOT_ID = "root"
        private const val QUEUE_ID = "queue"
        private const val RESUME_ID = "resume"
        private const val CHANNEL_ID = "solmusic_playback"
        private const val NOTIFICATION_ID = 4107
        private const val COMMAND_PREFERENCES = "solmusic-playback-command"
        private const val COMMAND_TOKEN_KEY = "token-v1"
        private val INTERNAL_ACTIONS = setOf(
            ACTION_UPDATE,
            ACTION_PLAY,
            ACTION_PAUSE,
            ACTION_NEXT,
            ACTION_PREVIOUS,
            ACTION_PLAY_FROM_MEDIA_ID,
            ACTION_DISMISS,
        )

        internal fun authorizedIntent(context: android.content.Context, action: String): Intent =
            Intent(context, PlaybackService::class.java)
                .setAction(action)
                .putExtra(EXTRA_COMMAND_TOKEN, commandToken(context))

        internal fun sanitizeArtworkUrl(value: String?): String? {
            val candidate = value?.trim()?.takeIf { it.isNotEmpty() && it.length <= 2_048 }
                ?: return null
            if (candidate.any { it.isISOControl() }) return null
            return try {
                val uri = Uri.parse(candidate)
                val scheme = uri.scheme?.lowercase()
                if (scheme !in setOf("http", "https") || uri.host.isNullOrBlank() || uri.userInfo != null) {
                    null
                } else {
                    candidate
                }
            } catch (_: Exception) {
                null
            }
        }

        private fun commandToken(context: android.content.Context): String {
            val preferences = context.getSharedPreferences(COMMAND_PREFERENCES, MODE_PRIVATE)
            preferences.getString(COMMAND_TOKEN_KEY, null)?.let { return it }
            val generated = UUID.randomUUID().toString()
            if (!preferences.edit().putString(COMMAND_TOKEN_KEY, generated).commit()) {
                throw IllegalStateException("Could not persist playback command authorization")
            }
            return generated
        }
    }

    private fun hasValidCommandToken(intent: Intent): Boolean {
        val expected = commandToken(this).toByteArray(Charsets.UTF_8)
        val provided = intent.getStringExtra(EXTRA_COMMAND_TOKEN)?.toByteArray(Charsets.UTF_8)
            ?: return false
        return MessageDigest.isEqual(expected, provided)
    }
}
