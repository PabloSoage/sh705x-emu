package com.varuna.sh705x.update

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.FileProvider
import androidx.core.net.toUri
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URI
import kotlin.concurrent.thread

/**
 * In-app updates: GitHub releases for the check, the APK for this device's ABI
 * downloaded into the cache, then an installer of the user's choosing.
 *
 * Ported from opendash (and Rustify before it), installer picker included: when more than one app on
 * the phone can install a package -- the system installer and a Shizuku-based
 * one such as Install With Options or SAI -- the dialog lists them rather than
 * handing the choice to a system chooser that may not show them all.
 *
 * No new dependency. HttpURLConnection and org.json are both in the platform.
 */
object AppUpdate {

    sealed interface Status {
        data object Idle : Status
        data object Checking : Status
        data object UpToDate : Status
        data object Failed : Status
        class Available(val update: Releases.Update) : Status
    }

    /** Where the last check got to. Process-wide, so a rebuilt screen keeps it. */
    var status by mutableStateOf<Status>(Status.Idle)
        private set

    /** Checked once per process when the app opens, and never twice at once. */
    private var checkedOnStart = false

    fun checkOnStart(context: Context) {
        if (checkedOnStart) return
        checkedOnStart = true
        check(context, manual = false)
    }

    /**
     * A manual check says when it fails; the one at start does not: a phone
     * without a connection is no error worth a message when it opens.
     */
    fun check(context: Context, manual: Boolean = true) {
        if (status == Status.Checking) return
        val previous = status
        status = Status.Checking
        val installed = installedVersion(context)
        thread(name = "update-check", isDaemon = true) {
            status = try {
                Releases.pick(fetch(), installed, Build.SUPPORTED_ABIS.toList())
                    ?.let { Status.Available(it) } ?: Status.UpToDate
            } catch (_: Exception) {
                if (manual) Status.Failed else previous
            }
        }
    }

    fun installedVersion(context: Context): String =
        runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }
            .getOrNull().orEmpty()

    /** Both endpoints, merged; only a failure of both is an error. See [Releases.pick]. */
    private fun fetch(): List<Releases.Release> {
        val listed = runCatching {
            val arr = JSONArray(get(Releases.LIST_API))
            (0 until arr.length()).mapNotNull { parse(arr.getJSONObject(it)) }
        }.getOrDefault(emptyList())
        val latest = runCatching { parse(JSONObject(get(Releases.LATEST_API))) }.getOrNull()
        if (listed.isEmpty() && latest == null) throw IOException("GitHub releases unavailable")
        return listOfNotNull(latest) + listed
    }

    private fun parse(o: JSONObject): Releases.Release? {
        val tag = o.optString("tag_name").takeIf { it.isNotBlank() } ?: return null
        val assets = o.optJSONArray("assets")
        return Releases.Release(
            tag = tag,
            title = o.optString("name").ifBlank { tag },
            body = o.optString("body").trim(),
            htmlUrl = o.optString("html_url").ifBlank { Releases.RELEASES_PAGE },
            assets = if (assets == null) emptyList() else (0 until assets.length()).mapNotNull { i ->
                val a = assets.getJSONObject(i)
                val url = a.optString("browser_download_url").takeIf { it.isNotBlank() } ?: return@mapNotNull null
                Releases.Asset(a.optString("name"), url, a.optLong("size"))
            },
            draft = o.optBoolean("draft", false),
        )
    }

    private fun get(url: String): String {
        val c = URI(url).toURL().openConnection() as HttpURLConnection
        try {
            c.connectTimeout = 15_000
            c.readTimeout = 30_000
            c.setRequestProperty("Accept", "application/vnd.github+json")
            c.setRequestProperty("User-Agent", USER_AGENT)
            if (c.responseCode !in 200..299) throw IOException("GitHub HTTP " + c.responseCode)
            return c.inputStream.use { String(it.readBytes(), Charsets.UTF_8) }
        } finally {
            c.disconnect()
        }
    }

    /** Downloads the APK into `cacheDir/updates/`, reporting 0..1, or -1 when the size is unknown. */
    fun download(context: Context, apk: Releases.Asset, onProgress: (Float) -> Unit): File {
        val dir = File(context.cacheDir, UPDATES_DIR).apply { mkdirs() }
        // Never keep an old APK around.
        dir.listFiles()?.forEach { it.delete() }
        val out = File(dir, apk.name)
        val c = URI(apk.url).toURL().openConnection() as HttpURLConnection
        try {
            c.connectTimeout = 15_000
            c.readTimeout = 30_000
            c.instanceFollowRedirects = true
            c.setRequestProperty("User-Agent", USER_AGENT)
            if (c.responseCode !in 200..299) throw IOException("HTTP " + c.responseCode)
            val total = c.contentLengthLong.takeIf { it > 0 } ?: apk.size.takeIf { it > 0 } ?: -1L
            c.inputStream.use { input ->
                out.outputStream().use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var done = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        output.write(buffer, 0, read)
                        done += read
                        onProgress(if (total > 0) (done.toFloat() / total).coerceIn(0f, 1f) else -1f)
                    }
                }
            }
        } catch (e: Exception) {
            out.delete()
            throw e
        } finally {
            c.disconnect()
        }
        return out
    }

    /** An app able to install a package: the system installer, Install With Options, SAI… */
    class Installer(val label: String, val packageName: String)

    private fun apkUri(context: Context, apk: File): Uri =
        FileProvider.getUriForFile(context, context.packageName + ".fileprovider", apk)

    private fun installIntent(uri: Uri): Intent =
        Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }

    /**
     * Every app on the phone that can install [apk], never this one. Needs the
     * package-archive `<queries>` entry in the manifest from Android 11.
     */
    fun installers(context: Context, apk: File): List<Installer> {
        val pm = context.packageManager
        return runCatching {
            pm.queryIntentActivities(installIntent(apkUri(context, apk)), 0)
                .mapNotNull { ri ->
                    val ai = ri.activityInfo ?: return@mapNotNull null
                    Installer(ri.loadLabel(pm).toString(), ai.packageName)
                }
                .filter { it.packageName != context.packageName }
                .distinctBy { it.packageName }
        }.getOrDefault(emptyList())
    }

    /** Install with the one the user picked, granting it read access to the file. */
    fun installWith(context: Context, apk: File, installer: Installer) {
        val uri = apkUri(context, apk)
        runCatching { context.grantUriPermission(installer.packageName, uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        runCatching { context.startActivity(installIntent(uri).setPackage(installer.packageName)) }
    }

    /** The system chooser, for when there is only one installer or none was picked. */
    fun install(context: Context, apk: File) {
        val chooser = Intent.createChooser(installIntent(apkUri(context, apk)), null)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(chooser) }
    }

    fun canInstall(context: Context): Boolean = context.packageManager.canRequestPackageInstalls()

    /** The per-app "install unknown apps" screen. */
    fun requestInstallPermission(context: Context) {
        val intent = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, ("package:" + context.packageName).toUri())
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(intent) }
    }

    fun openReleasePage(context: Context, update: Releases.Update) {
        val intent = Intent(Intent.ACTION_VIEW, update.htmlUrl.toUri()).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(intent) }
    }

    private const val UPDATES_DIR = "updates"
    private const val USER_AGENT = "sh705x-updater"
}
