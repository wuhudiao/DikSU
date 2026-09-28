package me.weishu.kernelsu.ui.util

/**
 * Changes the two things a user picks for a hidden install: the name the launcher shows and the
 * picture it shows. Both live in resources.arsc, and both are rewritten **in place**.
 *
 * In place is the whole design. A resource table is a series of chunks whose sizes and offsets have
 * to agree; growing the string pool moves every chunk after it, and Android's installer only
 * forgives that some of the time — a renamed APK that grew its pool installs, the same APK with an
 * icon added does not. Rewriting the bytes a string already occupies changes nothing about the
 * table's geometry, so there is nothing to get wrong:
 *
 *   the name   is padded with zero-width characters to exactly the byte length the label slot has
 *   the icon   is pointed at another path of exactly the same length, and the picture is added
 *              under that name
 *
 * Resource names are obfuscated in a release build, so a resource is addressed by its compiled id —
 * the same bits the R class carries — and by the entry index inside its type.
 */
object ResourcePatcher {

    private const val CHUNK_HEADER = 8
    private const val TABLE_HEADER = 12
    private const val PACKAGE_HEADER = 288
    private const val TYPE = 0x0201
    private const val FLAG_SPARSE = 0x01
    private const val NO_ENTRY = -1
    private const val TYPE_STRING = 0x03
    private const val ZERO_WIDTH = '\u200b'   // three bytes, invisible
    private const val NBSP = '\u00a0'         // two bytes, invisible enough

    /** One resource to rewrite: [newText] is handed the string it points at now. */
    class Target(val resourceId: Int, val newText: (String) -> String?)

    /** The rewritten table, plus the files that now have to exist for it. */
    class Result(val table: ByteArray, val files: Set<String>)

    fun patchInPlace(table: ByteArray, targets: List<Target>): Result? = runCatching {
        require(targets.isNotEmpty())
        val pool = Pool(table, TABLE_HEADER)
        val pkg = table.copyOfRange(TABLE_HEADER + pool.size, table.size)
        val files = mutableSetOf<String>()
        for (target in targets) {
            val typeId = (target.resourceId ushr 16) and 0xFF
            val entryIndex = target.resourceId and 0xFFFF
            forEachStringValue(pkg, typeId, entryIndex) { valueIndex ->
                val before = pool.get(table, valueIndex)
                val after = before?.let { target.newText(it) }
                if (after != null && rewrite(table, pool, valueIndex, after) && after.endsWith(".png")) {
                    files.add(after)
                }
            }
        }
        Result(table, files)
    }.getOrNull()

    /**
     * Where the picture should live instead of [old] — always a path of the same length, so the
     * entry pointing at it keeps its slot in the table.
     *
     * A copy that was customized before already points at our own file, and the new picture simply
     * replaces the bytes behind that same name; a plain build points at the adaptive icon's vector
     * XML, and the name is swapped for a `.png` of the same length.
     */
    fun iconPathLike(old: String): String? = when {
        old.endsWith(".png") -> old
        old.startsWith("res/") && old.endsWith(".xml") && old.length >= 8 ->
            "res/" + "x".repeat(old.length - 8) + ".png"
        else -> null
    }

    /** Calls [block] with the pool index behind every configuration of one resource. */
    private fun forEachStringValue(pkg: ByteArray, typeId: Int, entryIndex: Int, block: (Int) -> Unit) {
        var at = PACKAGE_HEADER
        while (at + CHUNK_HEADER <= pkg.size) {
            val type = readU16(pkg, at)
            val size = readU32(pkg, at + 4)
            if (size <= 0 || at + size > pkg.size) return
            if (type == TYPE && (pkg[at + 8].toInt() and 0xFF) == typeId) {
                val headerSize = readU16(pkg, at + 2)
                val entryCount = readU32(pkg, at + 12)
                val entriesStart = readU32(pkg, at + 16)
                val sparse = (pkg[at + 9].toInt() and FLAG_SPARSE) != 0
                var offset = NO_ENTRY
                if (sparse) {
                    for (i in 0 until entryCount) {
                        if (readU16(pkg, at + headerSize + 4 * i) == entryIndex) {
                            offset = readU16(pkg, at + headerSize + 4 * i + 2) * 4
                            break
                        }
                    }
                } else if (entryIndex < entryCount) {
                    offset = readU32(pkg, at + headerSize + 4 * entryIndex)
                }
                if (offset != NO_ENTRY && offset >= 0) {
                    val entryAt = at + entriesStart + offset
                    if (entryAt + 16 <= pkg.size) {
                        val valueAt = entryAt + readU16(pkg, entryAt)
                        if (valueAt + 8 <= pkg.size && (pkg[valueAt + 3].toInt() and 0xFF) == TYPE_STRING) {
                            block(readU32(pkg, valueAt + 4))
                        }
                    }
                }
            }
            at += size
        }
    }

    /**
     * Replaces the string at [index] with [text] padded to the same byte length. The entry's two
     * length prefixes, its data and its terminator keep their sizes, so the pool does not move.
     */
    private fun rewrite(table: ByteArray, pool: Pool, index: Int, text: String): Boolean {
        val start = TABLE_HEADER + pool.stringsStart + pool.offsets[index]
        val charPrefix = length8(table, start)
        val bytePrefix = length8(table, start + charPrefix.second)
        if (charPrefix.second != 1 || bytePrefix.second != 1) return false
        val dataAt = start + 2
        val oldBytes = bytePrefix.first
        val padded = padTo(text, oldBytes) ?: return false
        val raw = padded.toByteArray(Charsets.UTF_8)
        if (raw.size != oldBytes || padded.length >= 0x80) return false
        table[start] = padded.length.toByte()
        table[start + 1] = raw.size.toByte()
        raw.copyInto(table, dataAt)
        table[dataAt + raw.size] = 0
        return true
    }

    /** Pads with characters that occupy no visible width until the UTF-8 size is exactly [target]. */
    private fun padTo(text: String, target: Int): String? {
        var remaining = target - text.toByteArray(Charsets.UTF_8).size
        if (remaining < 0) return null
        val pad = StringBuilder()
        while (remaining >= 3) { pad.append(ZERO_WIDTH); remaining -= 3 }
        while (remaining >= 2) { pad.append(NBSP); remaining -= 2 }
        if (remaining == 1) { pad.append(' '); remaining = 0 }
        return if (remaining == 0) text + pad else null
    }

    /** The global string pool's layout, so a string can be found by the index an entry holds. */
    private class Pool(blob: ByteArray, at: Int) {
        val size = readU32(blob, at + 4)
        val count = readU32(blob, at + 8)
        val stringsStart = readU32(blob, at + 20)
        private val headerSize = readU16(blob, at + 2)
        private val utf8 = readU32(blob, at + 16) and 0x100 != 0
        val offsets = IntArray(count) { readU32(blob, at + headerSize + 4 * it) }

        fun get(blob: ByteArray, index: Int): String? {
            if (index < 0 || index >= count) return null
            var at = TABLE_HEADER + stringsStart + offsets[index]
            val charLength = length8(blob, at)
            at += charLength.second
            val byteLength = length8(blob, at)
            at += byteLength.second
            return if (utf8) {
                String(blob, at, byteLength.first, Charsets.UTF_8)
            } else {
                String(blob, at, byteLength.first * 2, Charsets.UTF_16LE)
            }
        }
    }

    /** A pool length as `value to bytesUsed`: one byte, or two when the first has its top bit set. */
    private fun length8(blob: ByteArray, at: Int): Pair<Int, Int> {
        val first = blob[at].toInt() and 0xFF
        return if (first and 0x80 != 0) {
            (((first and 0x7F) shl 8) or (blob[at + 1].toInt() and 0xFF)) to 2
        } else {
            first to 1
        }
    }

    private fun readU16(blob: ByteArray, at: Int) =
        (blob[at].toInt() and 0xFF) or ((blob[at + 1].toInt() and 0xFF) shl 8)

    private fun readU32(blob: ByteArray, at: Int): Int =
        readU16(blob, at) or (readU16(blob, at + 2) shl 16)
}
