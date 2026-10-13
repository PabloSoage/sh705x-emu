package com.varuna.sh705x.update

/**
 * Which GitHub release, if any, is newer than what is installed.
 *
 * The same logic as opendash's, Rustify's and openfuel's updaters, kept free of Android
 * and of JSON so it can be tested on the host: [AppUpdate] fetches and parses,
 * this decides.
 *
 * Releases are tagged like `v0.3.0-beta` and carry one APK per ABI, named after
 * it: `app-arm64-v8a-release.apk`, `app-x86_64-release.apk` (and a Windows zip, ignored).
 */
object Releases {

    const val OWNER = "PabloSoage"
    const val REPO = "sh705x-emu"
    const val RELEASES_PAGE = "https://github.com/$OWNER/$REPO/releases/latest"
    const val LIST_API = "https://api.github.com/repos/$OWNER/$REPO/releases?per_page=30"
    const val LATEST_API = "https://api.github.com/repos/$OWNER/$REPO/releases/latest"

    class Asset(val name: String, val url: String, val size: Long)

    class Release(
        val tag: String,
        val title: String,
        val body: String,
        val htmlUrl: String,
        val assets: List<Asset>,
        val draft: Boolean = false,
    )

    /** The newest release above the installed version, with the notes of every one missed. */
    class Update(
        val tag: String,
        val version: String,
        val title: String,
        /** Changelog from the installed version up to [tag], newest first. */
        val body: String,
        val htmlUrl: String,
        /** Null when no attached APK suits the device: the release page is the fallback. */
        val apk: Asset?,
    )

    private val VERSION = Regex("""(\d+)\.(\d+)(?:\.(\d+))?""")

    /** [major, minor, patch] from "v0.3.0-beta", "0.3.0" or "0.3"; empty when there is none. */
    fun parseVersion(s: String): List<Int> {
        val m = VERSION.find(s) ?: return emptyList()
        return listOf(m.groupValues[1].toInt(), m.groupValues[2].toInt(), m.groupValues[3].ifEmpty { "0" }.toInt())
    }

    fun isNewer(candidate: String, installed: String): Boolean {
        val a = parseVersion(candidate)
        val b = parseVersion(installed)
        if (a.isEmpty() || b.isEmpty()) return false
        return compare(a, b) > 0
    }

    private fun compare(a: List<Int>, b: List<Int>): Int =
        a.zip(b).firstOrNull { (x, y) -> x != y }?.let { (x, y) -> x.compareTo(y) } ?: 0

    /**
     * Null when up to date.
     *
     * [releases] is the union of both GitHub endpoints, in any order and with
     * duplicates. Neither is enough alone, and that was learned the hard way
     * in Rustify: the list is the only one with pre-releases and the only one
     * that gives a changelog across several versions, but GitHub served it
     * stale for a full day, topping out one version short, so an updater that
     * read only the list told everybody they were up to date. `latest` is
     * always current and hides pre-releases. Sorted by number here, not by the
     * order GitHub returns and not as text, where "0.10.0" sorts below "0.9.0".
     */
    fun pick(releases: List<Release>, installed: String, abis: List<String>): Update? {
        val missed = releases
            .filter { !it.draft }
            .distinctBy { it.tag }
            .filter { isNewer(it.tag, installed) }
            .sortedWith { x, y -> compare(parseVersion(y.tag), parseVersion(x.tag)) }
        val newest = missed.firstOrNull() ?: return null
        return Update(
            tag = newest.tag,
            version = parseVersion(newest.tag).joinToString("."),
            title = newest.title,
            body = if (missed.size == 1) {
                newest.body
            } else {
                missed.joinToString("\n\n") { "## ${it.title}\n\n${it.body}".trim() }
            },
            htmlUrl = newest.htmlUrl,
            apk = apkFor(newest.assets, abis),
        )
    }

    /** The APK named after the device's first supported ABI, else one with no ABI in its name. */
    fun apkFor(assets: List<Asset>, abis: List<String>): Asset? {
        val apks = assets.filter { it.name.endsWith(".apk", ignoreCase = true) }
        abis.forEach { abi -> apks.firstOrNull { it.name.contains(abi, ignoreCase = true) }?.let { return it } }
        return apks.firstOrNull { a -> KNOWN_ABIS.none { a.name.contains(it, ignoreCase = true) } }
    }

    private val KNOWN_ABIS = listOf("arm64-v8a", "armeabi", "x86_64", "x86")

    /** GitHub markdown as plain text: headings, bold, code, links and table rules without their marks. */
    fun plainNotes(md: String): String =
        md.replace(Regex("<!--.*?-->", RegexOption.DOT_MATCHES_ALL), "")
            .lineSequence()
            .filterNot { it.trim().matches(Regex("""\|?\s*:?-{3,}.*""")) }
            .joinToString("\n") { line ->
                line.trimEnd()
                    .replace(Regex("^#{1,6}\\s*"), "")
                    .replace(Regex("\\*\\*(.+?)\\*\\*"), "$1")
                    .replace(Regex("`([^`]+)`"), "$1")
                    .replace(Regex("""\[([^\]]+)]\([^)]+\)"""), "$1")
            }
            .replace(Regex("\n{3,}"), "\n\n")
            .trim()
}
