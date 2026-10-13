package com.varuna.sh705x.update

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ReleasesTest {

    private fun release(tag: String, body: String = "notes " + tag, draft: Boolean = false) =
        Releases.Release(
            tag = tag,
            title = "Beta " + tag,
            body = body,
            htmlUrl = "https://example.invalid/" + tag,
            assets = listOf(
                Releases.Asset("app-arm64-v8a-release.apk", "u/arm", 1),
                Releases.Asset("app-x86_64-release.apk", "u/x86", 1),
                Releases.Asset("baselineProfiles.zip", "u/zip", 1),
            ),
            draft = draft,
        )

    @Test
    fun versionsCompareByNumberNotText() {
        assertTrue(Releases.isNewer("v0.10.0-beta", "0.9.0"))
        assertFalse(Releases.isNewer("v0.3.0-beta", "0.3.0"))
        assertTrue(Releases.isNewer("v0.3.1-beta", "0.3.0"))
        assertFalse(Releases.isNewer("nightly", "0.3.0"))
    }

    @Test
    fun upToDateIsNull() {
        assertNull(Releases.pick(listOf(release("v0.3.0-beta"), release("v0.2.0-beta")), "0.3.0", listOf("arm64-v8a")))
    }

    @Test
    fun theNewestWinsWhateverOrderGitHubUsesAndNotesCoverEveryMissedRelease() {
        // Duplicated, as the union of the two endpoints is.
        val all = listOf(release("v0.5.0-beta"), release("v0.4.0-beta"), release("v0.10.0-beta"), release("v0.5.0-beta"))
        val u = Releases.pick(all, "0.3.0", listOf("arm64-v8a"))!!
        assertEquals("v0.10.0-beta", u.tag)
        assertEquals("0.10.0", u.version)
        // Newest first, each once.
        val order = listOf("notes v0.10.0-beta", "notes v0.5.0-beta", "notes v0.4.0-beta").map { u.body.indexOf(it) }
        assertTrue(order.all { it >= 0 } && order == order.sorted())
        assertEquals(1, Regex("notes v0.5.0-beta").findAll(u.body).count())
    }

    @Test
    fun draftsAreNeverOffered() {
        assertNull(Releases.pick(listOf(release("v9.0.0", draft = true)), "0.3.0", listOf("arm64-v8a")))
    }

    @Test
    fun theApkIsTheOneForThisDevice() {
        val u = Releases.pick(listOf(release("v0.4.0-beta")), "0.3.0", listOf("x86_64", "arm64-v8a"))!!
        assertEquals("app-x86_64-release.apk", u.apk!!.name)
        val none = Releases.pick(listOf(release("v0.4.0-beta")), "0.3.0", listOf("armeabi-v7a"))!!
        assertNull("no matching APK means the release page, not a wrong APK", none.apk)
    }

    @Test
    fun notesLoseTheirMarkdown() {
        val md = "## What changed\n\n| a | b |\n|---|---|\n**Bold** and `code` and [a link](https://x)\n<!-- hidden -->"
        assertEquals("What changed\n\n| a | b |\nBold and code and a link", Releases.plainNotes(md))
    }
}
