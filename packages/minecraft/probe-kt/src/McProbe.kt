/**
 * Assert Minecraft Server List Ping responses, from Kotlin.
 *
 * The JVM twin of mc-probe (../probe/mc_probe.py), with the same flags and
 * exit-code contract: a zero exit means the server answered the SLP
 * exchange and every requested assertion held; any failure is named on
 * stderr so health-check runners can surface it. It speaks the wire format
 * through `McProtocolJvm`, the unibind-rendered Java class over the Rust
 * `mc-protocol` crate, so the Python probe, this probe, and the servers'
 * tests share one protocol implementation.
 *
 * Addresses are explicit `host[:port]` — no SRV lookup (in-repo checks and
 * tests address servers directly; resolve SRV records before calling in).
 */

import kotlin.system.exitProcess

private const val USAGE =
    """usage: mc-probe-kt ADDRESS [--motd-contains SUBSTRING]...
       [--protocol-version N] [--min-max-players N] [--timeout SECONDS]"""

private data class Args(
    val address: String,
    val motdContains: List<String>,
    val protocolVersion: Int?,
    val minMaxPlayers: Long?,
    val timeoutSeconds: Double,
)

private fun usageError(message: String): Nothing {
    System.err.println("mc-probe-kt: $message")
    System.err.println(USAGE)
    exitProcess(2)
}

private fun parseArgs(argv: Array<String>): Args {
    var address: String? = null
    val motdContains = mutableListOf<String>()
    var protocolVersion: Int? = null
    var minMaxPlayers: Long? = null
    var timeoutSeconds = 5.0

    var index = 0
    fun value(flag: String): String {
        index += 1
        if (index >= argv.size) usageError("$flag needs a value")
        return argv[index]
    }
    while (index < argv.size) {
        when (val argument = argv[index]) {
            "--motd-contains" -> motdContains += value(argument)
            "--protocol-version" ->
                protocolVersion =
                    value(argument).toIntOrNull()
                        ?: usageError("--protocol-version needs an integer")
            "--min-max-players" ->
                minMaxPlayers =
                    value(argument).toLongOrNull()
                        ?: usageError("--min-max-players needs an integer")
            "--timeout" ->
                timeoutSeconds =
                    value(argument).toDoubleOrNull() ?: usageError("--timeout needs a number")
            "--help", "-h" -> {
                println(USAGE)
                exitProcess(0)
            }
            else ->
                when {
                    argument.startsWith("-") -> usageError("unknown flag $argument")
                    address != null -> usageError("more than one address given")
                    else -> address = argument
                }
        }
        index += 1
    }
    return Args(
        address = address ?: usageError("an address is required"),
        motdContains = motdContains,
        protocolVersion = protocolVersion,
        minMaxPlayers = minMaxPlayers,
        timeoutSeconds = timeoutSeconds,
    )
}

fun main(argv: Array<String>) {
    val args = parseArgs(argv)

    val status =
        try {
            McProtocolJvm.status(args.address, args.timeoutSeconds)
        } catch (exception: McProtocolJvm.SlpException) {
            System.err.println("mc-probe-kt: SLP failed for ${args.address}: ${exception.message}")
            exitProcess(1)
        }

    val failures = buildList {
        val plain = McProtocolJvm.stripFormatCodes(status.motd())
        for (needle in args.motdContains) {
            if (McProtocolJvm.stripFormatCodes(needle) !in plain) {
                add("motd missing substring \"$needle\" (got \"$plain\")")
            }
        }
        args.protocolVersion?.let { expected ->
            if (status.protocolVersion() != expected) {
                add("protocol version ${status.protocolVersion()} does not match expected $expected")
            }
        }
        args.minMaxPlayers?.let { minimum ->
            if (status.playersMax() < minimum) {
                add("max players ${status.playersMax()} below required $minimum")
            }
        }
    }
    if (failures.isNotEmpty()) {
        for (failure in failures) {
            System.err.println("mc-probe-kt: $failure")
        }
        exitProcess(1)
    }

    println(
        "mc-probe-kt: ${args.address} ok " +
            "(version=\"${status.versionName()}\", " +
            "protocol=${status.protocolVersion()}, " +
            "players=${status.playersOnline()}/${status.playersMax()})"
    )
}
