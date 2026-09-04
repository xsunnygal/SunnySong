package app.solmusic.storage

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.media.app.NotificationCompat.MediaStyle
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import java.net.URL
import java.util.concurrent.Executors

class PlaybackService : Service() {
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
    private var playback: PlaybackInfo? = null
    private var artwork: Bitmap? = null
    private var loadedArtworkUrl: String? = null

    override fun onCreate() {
        super.onCreate()
        isRunning = true
        createNotificationChannel()
        mediaSession = MediaSessionCompat(this, "SunnySongPlayback").apply {
            setCallback(object : MediaSessionCompat.Callback() {
                override fun onPlay() = dispatch(CONTROL_PLAY)
                override fun onPause() = dispatch(CONTROL_PAUSE)
                override fun onSkipToNext() = dispatch(CONTROL_NEXT)
                override fun onSkipToPrevious() = dispatch(CONTROL_PREVIOUS)
                override fun onSeekTo(position: Long) = dispatch(CONTROL_SEEK, position)
                override fun onStop() = dispatch(CONTROL_PAUSE)
            })
            setSessionActivity(openAppIntent())
            isActive = true
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_UPDATE -> updateFrom(intent)
            ACTION_PLAY -> dispatch(CONTROL_PLAY)
            ACTION_PAUSE -> dispatch(CONTROL_PAUSE)
            ACTION_NEXT -> dispatch(CONTROL_NEXT)
            ACTION_PREVIOUS -> dispatch(CONTROL_PREVIOUS)
            ACTION_DISMISS -> {
                dispatch(CONTROL_PAUSE)
                stopPlaybackService()
                return START_NOT_STICKY
            }
        }
        return if (playback == null) START_NOT_STICKY else START_STICKY
    }

    private fun updateFrom(intent: Intent) {
        if (!intent.getBooleanExtra(EXTRA_ACTIVE, false)) {
            stopPlaybackService()
            return
        }
        playback = PlaybackInfo(
            title = intent.getStringExtra(EXTRA_TITLE).orEmpty(),
            artist = intent.getStringExtra(EXTRA_ARTIST).orEmpty(),
            album = intent.getStringExtra(EXTRA_ALBUM),
            artworkUrl = intent.getStringExtra(EXTRA_ARTWORK_URL),
            playing = intent.getBooleanExtra(EXTRA_PLAYING, false),
            positionMs = intent.getLongExtra(EXTRA_POSITION_MS, 0L).coerceAtLeast(0L),
            durationMs = intent.getLongExtra(EXTRA_DURATION_MS, 0L).coerceAtLeast(0L),
            canGoPrevious = intent.getBooleanExtra(EXTRA_CAN_GO_PREVIOUS, false),
            canGoNext = intent.getBooleanExtra(EXTRA_CAN_GO_NEXT, false),
        )
        updateMediaSession()
        startForeground(NOTIFICATION_ID, buildNotification())
        loadArtwork(playback?.artworkUrl)
    }

    private fun dispatch(action: String, positionMs: Long? = null) {
        val current = playback ?: return
        playback = when (action) {
            CONTROL_PLAY -> current.copy(playing = true)
            CONTROL_PAUSE -> current.copy(playing = false)
            CONTROL_SEEK -> current.copy(positionMs = positionMs ?: current.positionMs)
            else -> current
        }
        StoragePlugin.dispatchMediaControl(action, positionMs)
        updateMediaSession()
        NotificationManagerCompat.from(this).notify(NOTIFICATION_ID, buildNotification())
    }

    private fun updateMediaSession() {
        val current = playback ?: return
        var actions = PlaybackStateCompat.ACTION_PLAY or
            PlaybackStateCompat.ACTION_PAUSE or
            PlaybackStateCompat.ACTION_PLAY_PAUSE or
            PlaybackStateCompat.ACTION_SEEK_TO or
            PlaybackStateCompat.ACTION_STOP
        if (current.canGoPrevious) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS
        if (current.canGoNext) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_NEXT
        mediaSession.setPlaybackState(
            PlaybackStateCompat.Builder()
                .setActions(actions)
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
        mediaSession.isActive = true
    }

    private fun buildNotification(): android.app.Notification {
        val current = requireNotNull(playback)
        val previous = servicePendingIntent(ACTION_PREVIOUS, 1)
        val toggleAction = if (current.playing) ACTION_PAUSE else ACTION_PLAY
        val toggleIcon = if (current.playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play
        val toggleLabel = if (current.playing) "Pause" else "Play"
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
            .addAction(android.R.drawable.ic_media_previous, "Previous", previous)
            .addAction(toggleIcon, toggleLabel, toggle)
            .addAction(android.R.drawable.ic_media_next, "Next", next)
            .setStyle(
                MediaStyle()
                    .setMediaSession(mediaSession.sessionToken)
                    .setShowActionsInCompactView(0, 1, 2)
                    .setShowCancelButton(false)
            )
            .build()
    }

    private fun loadArtwork(url: String?) {
        if (url.isNullOrBlank() || url == loadedArtworkUrl) return
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
            playback?.let {
                NotificationManagerCompat.from(this).notify(NOTIFICATION_ID, buildNotification())
            }
        }
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Music playback",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Playback controls for SunnySong"
            setShowBadge(false)
        }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun servicePendingIntent(action: String, requestCode: Int): PendingIntent {
        return PendingIntent.getService(
            this,
            requestCode,
            Intent(this, PlaybackService::class.java).setAction(action),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

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
        playback = null
        mediaSession.isActive = false
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        stopSelf()
    }

    override fun onDestroy() {
        isRunning = false
        artworkExecutor.shutdownNow()
        mediaSession.release()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        const val ACTION_UPDATE = "app.solmusic.playback.UPDATE"
        const val ACTION_PLAY = "app.solmusic.playback.PLAY"
        const val ACTION_PAUSE = "app.solmusic.playback.PAUSE"
        const val ACTION_NEXT = "app.solmusic.playback.NEXT"
        const val ACTION_PREVIOUS = "app.solmusic.playback.PREVIOUS"
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

        const val CONTROL_PLAY = "play"
        const val CONTROL_PAUSE = "pause"
        const val CONTROL_NEXT = "next"
        const val CONTROL_PREVIOUS = "previous"
        const val CONTROL_SEEK = "seek"

        @Volatile
        var isRunning: Boolean = false
            private set

        private const val CHANNEL_ID = "solmusic_playback"
        private const val NOTIFICATION_ID = 4107
    }
}
