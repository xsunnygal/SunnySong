package app.solmusic.storage

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import androidx.documentfile.provider.DocumentFile
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.security.KeyStore
import java.util.ArrayDeque
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec


@InvokeArg
class PublishAudioArgs {
    lateinit var treeUri: String
    lateinit var displayName: String
    lateinit var mimeType: String
    lateinit var sourcePath: String
}

@InvokeArg
class SecretArgs {
    lateinit var key: String
    lateinit var value: String
}

@InvokeArg
class SecretKeyArgs {
    lateinit var key: String
}

@InvokeArg
class MediaSessionArgs {
    var active: Boolean = false
    var title: String = ""
    var artist: String = ""
    var album: String? = null
    var artworkUrl: String? = null
    var playing: Boolean = false
    var positionMs: Long = 0
    var durationMs: Long = 0
    var canGoPrevious: Boolean = false
    var canGoNext: Boolean = false
}

@TauriPlugin
class StoragePlugin(private val activity: Activity) : Plugin(activity) {
    init {
        currentPlugin = this
    }

    @Command
    override fun registerListener(invoke: Invoke) {
        super.registerListener(invoke)
        flushPendingMediaControls()
    }

    @Command
    fun pickDirectory(invoke: Invoke) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            addFlags(Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
            addFlags(Intent.FLAG_GRANT_PREFIX_URI_PERMISSION)
        }
        startActivityForResult(invoke, intent, "directoryPicked")
    }

    @ActivityCallback
    fun directoryPicked(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode != Activity.RESULT_OK) {
            invoke.reject("Directory selection was cancelled")
            return
        }
        val uri = result.data?.data
        if (uri == null) {
            invoke.reject("Android did not return a directory")
            return
        }
        try {
            val grantFlags = (result.data?.flags ?: 0) and
                (Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            activity.contentResolver.takePersistableUriPermission(uri, grantFlags)
            val directory = DocumentFile.fromTreeUri(activity, uri)
            val response = JSObject().apply {
                put("uri", uri.toString())
                put("displayName", directory?.name ?: "Selected folder")
                put("canWrite", directory?.canWrite() == true)
                put("persisted", activity.contentResolver.persistedUriPermissions.any { it.uri == uri && it.isWritePermission })
            }
            invoke.resolve(response)
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Could not persist directory access")
        }
    }

    @Command
    fun publishAudio(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(PublishAudioArgs::class.java)
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Invalid audio export request")
            return
        }
        Thread {
            var destination: DocumentFile? = null
            try {
                val treeUri = Uri.parse(args.treeUri)
                val permission = activity.contentResolver.persistedUriPermissions
                    .firstOrNull { it.uri == treeUri && it.isWritePermission }
                    ?: throw IllegalStateException("Access to this folder was revoked. Choose it again.")
                if (!permission.isWritePermission) throw IllegalStateException("The selected folder is read-only")
                val directory = DocumentFile.fromTreeUri(activity, treeUri)
                    ?: throw IllegalStateException("The selected folder is unavailable")
                if (!directory.exists() || !directory.isDirectory || !directory.canWrite()) {
                    throw IllegalStateException("The selected folder is unavailable or read-only")
                }
                val source = File(args.sourcePath)
                if (!source.isFile) throw IllegalStateException("The completed audio file is unavailable")
                val displayName = uniqueName(directory, args.displayName)
                destination = directory.createFile(args.mimeType, displayName)
                    ?: throw IllegalStateException("Android could not create the audio file")
                activity.contentResolver.openOutputStream(destination.uri, "w").use { output ->
                    if (output == null) throw IllegalStateException("Android could not open the destination")
                    source.inputStream().use { input -> input.copyTo(output, 128 * 1024) }
                }
                val response = JSObject().apply { put("uri", destination.uri.toString()) }
                invoke.resolve(response)
            } catch (error: Exception) {
                destination?.delete()
                invoke.reject(error.message ?: "Could not save audio to the selected folder")
            }
        }.start()
    }

    @Command
    fun storeSecret(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(SecretArgs::class.java)
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Invalid secure storage request")
            return
        }
        try {
            requireValidSecretKey(args.key)
            val cipher = Cipher.getInstance(SECRET_TRANSFORMATION)
            cipher.init(Cipher.ENCRYPT_MODE, secretKey())
            val encrypted = cipher.doFinal(args.value.toByteArray(Charsets.UTF_8))
            val payload = ByteArray(cipher.iv.size + encrypted.size)
            cipher.iv.copyInto(payload)
            encrypted.copyInto(payload, cipher.iv.size)
            val saved = securePreferences().edit()
                .putString(args.key, Base64.encodeToString(payload, Base64.NO_WRAP))
                .commit()
            if (!saved) throw IllegalStateException("Android did not persist the encrypted credential")
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Could not securely store credential")
        }
    }

    @Command
    fun getSecret(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(SecretKeyArgs::class.java)
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Invalid secure storage request")
            return
        }
        try {
            requireValidSecretKey(args.key)
            val encoded = securePreferences().getString(args.key, null)
            val response = JSObject()
            if (encoded != null) {
                val payload = Base64.decode(encoded, Base64.NO_WRAP)
                if (payload.size <= GCM_IV_BYTES) throw IllegalStateException("Stored credential is invalid")
                val cipher = Cipher.getInstance(SECRET_TRANSFORMATION)
                cipher.init(
                    Cipher.DECRYPT_MODE,
                    secretKey(),
                    GCMParameterSpec(GCM_TAG_BITS, payload.copyOfRange(0, GCM_IV_BYTES)),
                )
                val cleartext = cipher.doFinal(payload.copyOfRange(GCM_IV_BYTES, payload.size))
                response.put("value", cleartext.toString(Charsets.UTF_8))
            }
            invoke.resolve(response)
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Could not read secure credential")
        }
    }

    @Command
    fun deleteSecret(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(SecretKeyArgs::class.java)
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Invalid secure storage request")
            return
        }
        try {
            requireValidSecretKey(args.key)
            val removed = securePreferences().edit().remove(args.key).commit()
            if (!removed) throw IllegalStateException("Android did not remove the encrypted credential")
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Could not delete secure credential")
        }
    }

    @Command
    fun backgroundApp(invoke: Invoke) {
        activity.moveTaskToBack(true)
        invoke.resolve()
    }

    @Command
    fun updateMediaSession(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(MediaSessionArgs::class.java)
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Invalid media session update")
            return
        }
        val intent = Intent(activity, PlaybackService::class.java).apply {
            action = PlaybackService.ACTION_UPDATE
            putExtra(PlaybackService.EXTRA_ACTIVE, args.active)
            putExtra(PlaybackService.EXTRA_TITLE, args.title)
            putExtra(PlaybackService.EXTRA_ARTIST, args.artist)
            putExtra(PlaybackService.EXTRA_ALBUM, args.album)
            putExtra(PlaybackService.EXTRA_ARTWORK_URL, args.artworkUrl)
            putExtra(PlaybackService.EXTRA_PLAYING, args.playing)
            putExtra(PlaybackService.EXTRA_POSITION_MS, args.positionMs)
            putExtra(PlaybackService.EXTRA_DURATION_MS, args.durationMs)
            putExtra(PlaybackService.EXTRA_CAN_GO_PREVIOUS, args.canGoPrevious)
            putExtra(PlaybackService.EXTRA_CAN_GO_NEXT, args.canGoNext)
        }
        try {
            if (args.active && !PlaybackService.isRunning) {
                ContextCompat.startForegroundService(activity, intent)
            } else {
                activity.startService(intent)
            }
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Could not update Android media controls")
        }
    }

    private fun securePreferences() =
        activity.getSharedPreferences(SECRET_PREFERENCES, Activity.MODE_PRIVATE)

    private fun secretKey(): SecretKey {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (keyStore.getKey(SECRET_KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(
                SECRET_KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return generator.generateKey()
    }

    private fun requireValidSecretKey(key: String) {
        require(key.matches(Regex("[A-Za-z0-9._-]{1,80}"))) { "Invalid secure credential key" }
    }

    private fun uniqueName(directory: DocumentFile, requested: String): String {
        if (directory.findFile(requested) == null) return requested
        val dot = requested.lastIndexOf('.')
        val stem = if (dot > 0) requested.substring(0, dot) else requested
        val extension = if (dot > 0) requested.substring(dot) else ""
        var suffix = 1
        while (directory.findFile("$stem ($suffix)$extension") != null) suffix += 1
        return "$stem ($suffix)$extension"
    }

    private fun flushPendingMediaControls() {
        if (!hasListener(MEDIA_CONTROL_EVENT)) return
        val controls = synchronized(pendingMediaControls) {
            buildList {
                while (pendingMediaControls.isNotEmpty()) add(pendingMediaControls.removeFirst())
            }
        }
        for ((action, positionMs) in controls) triggerMediaControl(action, positionMs)
    }

    private fun triggerMediaControl(action: String, positionMs: Long?) {
        val payload = JSObject().apply {
            put("action", action)
            if (positionMs != null) put("positionMs", positionMs)
        }
        trigger(MEDIA_CONTROL_EVENT, payload)
    }

    companion object {
        private const val MEDIA_CONTROL_EVENT = "media-control"
        private const val MAX_PENDING_MEDIA_CONTROLS = 16
        private const val SECRET_PREFERENCES = "solmusic-secure-storage"
        private const val SECRET_KEY_ALIAS = "app.solmusic.secure-storage"
        private const val SECRET_TRANSFORMATION = "AES/GCM/NoPadding"
        private const val GCM_IV_BYTES = 12
        private const val GCM_TAG_BITS = 128

        @Volatile
        private var currentPlugin: StoragePlugin? = null
        private val pendingMediaControls = ArrayDeque<Pair<String, Long?>>()

        fun dispatchMediaControl(action: String, positionMs: Long? = null) {
            val plugin = currentPlugin
            if (plugin == null) {
                enqueueMediaControl(action, positionMs)
                return
            }
            plugin.activity.runOnUiThread {
                if (plugin.hasListener(MEDIA_CONTROL_EVENT)) {
                    plugin.triggerMediaControl(action, positionMs)
                } else {
                    enqueueMediaControl(action, positionMs)
                }
            }
        }

        private fun enqueueMediaControl(action: String, positionMs: Long?) {
            synchronized(pendingMediaControls) {
                while (pendingMediaControls.size >= MAX_PENDING_MEDIA_CONTROLS) {
                    pendingMediaControls.removeFirst()
                }
                pendingMediaControls.addLast(action to positionMs)
            }
        }
    }
}
