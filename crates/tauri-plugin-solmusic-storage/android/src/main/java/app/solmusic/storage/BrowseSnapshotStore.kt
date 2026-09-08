package app.solmusic.storage

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.security.MessageDigest

internal object BrowseSnapshotStore {
    data class Item(
        val sourceId: String,
        val mediaId: String,
        val title: String,
        val artist: String,
        val album: String?,
        val artworkUrl: String?,
        val durationMs: Long,
    )

    data class Snapshot(
        val queue: List<Item>,
        val current: Item?,
        val currentIndex: Int?,
    )

    fun create(
        queue: List<MediaSessionItemArgs>,
        current: MediaSessionItemArgs?,
        currentIndex: Int?,
    ): Snapshot {
        val boundedQueue = queue.take(MAX_QUEUE_ITEMS).mapNotNull(::itemFrom)
        val currentItem = current?.let(::itemFrom)
        return Snapshot(
            queue = boundedQueue,
            current = currentItem,
            currentIndex = currentIndex?.takeIf { it >= 0 },
        )
    }

    fun save(context: Context, snapshot: Snapshot) {
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY_SNAPSHOT, toJson(snapshot).toString())
            .commit()
    }

    fun load(context: Context): Snapshot {
        val raw = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .getString(KEY_SNAPSHOT, null) ?: return Snapshot(emptyList(), null, null)
        return try {
            fromJson(JSONObject(raw))
        } catch (_: Exception) {
            Snapshot(emptyList(), null, null)
        }
    }

    private fun itemFrom(value: MediaSessionItemArgs): Item? {
        val sourceId = value.id.trim().take(MAX_ID_LENGTH)
        if (sourceId.isEmpty()) return null
        return Item(
            sourceId = sourceId,
            mediaId = mediaId(sourceId),
            title = value.title.trim().take(MAX_TEXT_LENGTH).ifEmpty { "SunnySong" },
            artist = value.artist.trim().take(MAX_TEXT_LENGTH),
            album = value.album?.trim()?.take(MAX_TEXT_LENGTH)?.ifEmpty { null },
            artworkUrl = value.artworkUrl?.trim()?.take(MAX_URL_LENGTH)?.ifEmpty { null },
            durationMs = value.durationMs.coerceAtLeast(0L),
        )
    }

    private fun mediaId(sourceId: String): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(sourceId.toByteArray(Charsets.UTF_8))
        return "song:" + digest.take(16).joinToString("") {
            (it.toInt() and 0xff).toString(16).padStart(2, '0')
        }
    }

    private fun toJson(snapshot: Snapshot) = JSONObject().apply {
        put("queue", JSONArray().apply { snapshot.queue.forEach { put(itemToJson(it)) } })
        put("current", snapshot.current?.let(::itemToJson) ?: JSONObject.NULL)
        put("currentIndex", snapshot.currentIndex ?: JSONObject.NULL)
    }

    private fun itemToJson(item: Item) = JSONObject().apply {
        put("sourceId", item.sourceId)
        put("mediaId", item.mediaId)
        put("title", item.title)
        put("artist", item.artist)
        put("album", item.album ?: JSONObject.NULL)
        put("artworkUrl", item.artworkUrl ?: JSONObject.NULL)
        put("durationMs", item.durationMs)
    }

    private fun fromJson(value: JSONObject): Snapshot {
        val queueJson = value.optJSONArray("queue") ?: JSONArray()
        val queue = buildList {
            for (index in 0 until minOf(queueJson.length(), MAX_QUEUE_ITEMS)) {
                queueJson.optJSONObject(index)?.let(::itemFromJson)?.let(::add)
            }
        }
        val current = value.optJSONObject("current")?.let(::itemFromJson)
        val currentIndex = if (value.isNull("currentIndex")) null else value.optInt("currentIndex")
        return Snapshot(queue, current, currentIndex?.takeIf { it >= 0 })
    }

    private fun itemFromJson(value: JSONObject): Item? {
        val sourceId = value.optString("sourceId").take(MAX_ID_LENGTH)
        val mediaId = value.optString("mediaId").take(MAX_ID_LENGTH)
        if (sourceId.isBlank() || mediaId.isBlank()) return null
        return Item(
            sourceId = sourceId,
            mediaId = mediaId,
            title = value.optString("title", "SunnySong").take(MAX_TEXT_LENGTH),
            artist = value.optString("artist").take(MAX_TEXT_LENGTH),
            album = value.optNullableString("album")?.take(MAX_TEXT_LENGTH),
            artworkUrl = value.optNullableString("artworkUrl")?.take(MAX_URL_LENGTH),
            durationMs = value.optLong("durationMs").coerceAtLeast(0L),
        )
    }

    private fun JSONObject.optNullableString(key: String): String? =
        if (isNull(key)) null else optString(key).ifBlank { null }

    private const val PREFERENCES = "solmusic-media-browser"
    private const val KEY_SNAPSHOT = "browse-snapshot-v1"
    private const val MAX_QUEUE_ITEMS = 40
    private const val MAX_ID_LENGTH = 512
    private const val MAX_TEXT_LENGTH = 300
    private const val MAX_URL_LENGTH = 2_048
}
