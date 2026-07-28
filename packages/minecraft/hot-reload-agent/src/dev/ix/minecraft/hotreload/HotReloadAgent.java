package dev.ix.minecraft.hotreload;

import java.io.BufferedReader;
import java.io.BufferedWriter;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.lang.instrument.ClassDefinition;
import java.lang.instrument.Instrumentation;
import java.net.StandardProtocolFamily;
import java.net.UnixDomainSocketAddress;
import java.nio.channels.Channels;
import java.nio.channels.ServerSocketChannel;
import java.nio.channels.SocketChannel;
import java.nio.charset.StandardCharsets;
import java.nio.file.DirectoryStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;
import java.util.jar.JarEntry;
import java.util.jar.JarInputStream;

public final class HotReloadAgent {
    private static volatile Instrumentation instrumentation;

    private HotReloadAgent() {
    }

    public static void premain(String args, Instrumentation inst) {
        start(args, inst);
    }

    public static void agentmain(String args, Instrumentation inst) {
        start(args, inst);
    }

    private static void start(String args, Instrumentation inst) {
        instrumentation = inst;
        Path socketPath = Path.of(option(args, "socket", "/run/minecraft-hot-reload/socket"));
        Thread thread = new Thread(() -> serve(socketPath), "ix-minecraft-hot-reload");
        thread.setDaemon(true);
        thread.start();
    }

    private static String option(String args, String name, String fallback) {
        if (args == null || args.isBlank()) {
            return fallback;
        }

        for (String part : args.split(",")) {
            String prefix = name + "=";
            if (part.startsWith(prefix)) {
                return part.substring(prefix.length());
            }
        }

        return fallback;
    }

    private static void serve(Path socketPath) {
        try {
            Files.createDirectories(socketPath.getParent());
            Files.deleteIfExists(socketPath);
            try (ServerSocketChannel server = ServerSocketChannel.open(StandardProtocolFamily.UNIX)) {
                server.bind(UnixDomainSocketAddress.of(socketPath));
                while (true) {
                    try (SocketChannel client = server.accept()) {
                        handle(client);
                    } catch (Throwable err) {
                        err.printStackTrace(System.err);
                    }
                }
            }
        } catch (Throwable err) {
            err.printStackTrace(System.err);
        }
    }

    private static void handle(SocketChannel client) throws IOException {
        BufferedReader reader =
            new BufferedReader(new InputStreamReader(Channels.newInputStream(client), StandardCharsets.UTF_8));
        BufferedWriter writer =
            new BufferedWriter(new OutputStreamWriter(Channels.newOutputStream(client), StandardCharsets.UTF_8));

        String line = reader.readLine();
        if (line == null || line.isBlank()) {
            write(writer, "ERR empty command");
            return;
        }

        String[] parts = line.split(" ", 2);
        try {
            switch (parts[0]) {
                case "PING" -> write(writer, "OK pong");
                case "REDEFINE_DIR" -> {
                    if (parts.length != 2) {
                        write(writer, "ERR REDEFINE_DIR requires a directory");
                    } else {
                        write(writer, redefineDirectory(Path.of(parts[1])));
                    }
                }
                default -> write(writer, "ERR unknown command " + parts[0]);
            }
        } catch (Throwable err) {
            write(writer, "ERR " + err.getClass().getSimpleName() + ": " + err.getMessage());
        }
    }

    private static void write(BufferedWriter writer, String line) throws IOException {
        writer.write(line);
        writer.newLine();
        writer.flush();
    }

    private static String redefineDirectory(Path dir) throws Exception {
        if (instrumentation == null) {
            return "ERR instrumentation is not available";
        }
        if (!Files.isDirectory(dir)) {
            return "ERR not a directory: " + dir;
        }

        int jars = 0;
        int classes = 0;
        int failures = 0;
        StringBuilder failureText = new StringBuilder();
        Map<String, Class<?>> loaded = loadedClasses();

        try (DirectoryStream<Path> stream = Files.newDirectoryStream(dir, "*.jar")) {
            for (Path jar : stream) {
                jars++;
                try (JarInputStream input = new JarInputStream(Files.newInputStream(jar))) {
                    JarEntry entry;
                    while ((entry = input.getNextJarEntry()) != null) {
                        String name = entry.getName();
                        if (entry.isDirectory() || !name.endsWith(".class") || name.equals("module-info.class")) {
                            continue;
                        }

                        String className = name.substring(0, name.length() - ".class".length()).replace('/', '.');
                        Class<?> loadedClass = loaded.get(className);
                        if (loadedClass == null || !instrumentation.isModifiableClass(loadedClass)) {
                            continue;
                        }

                        byte[] bytes = readAll(input);
                        try {
                            instrumentation.redefineClasses(new ClassDefinition(loadedClass, bytes));
                            classes++;
                        } catch (Throwable err) {
                            failures++;
                            if (failureText.length() < 2048) {
                                failureText.append(className)
                                    .append(": ")
                                    .append(err.getClass().getSimpleName())
                                    .append(": ")
                                    .append(err.getMessage())
                                    .append("; ");
                            }
                        }
                    }
                }
            }
        }

        if (failures > 0) {
            return "ERR redefined " + classes + " classes from " + jars + " jars; " + failures
                + " failures: " + failureText;
        }
        return "OK redefined " + classes + " classes from " + jars + " jars";
    }

    private static Map<String, Class<?>> loadedClasses() {
        Map<String, Class<?>> loaded = new HashMap<>();
        for (Class<?> klass : instrumentation.getAllLoadedClasses()) {
            loaded.putIfAbsent(klass.getName(), klass);
        }
        return loaded;
    }

    private static byte[] readAll(InputStream input) throws IOException {
        return input.readAllBytes();
    }

    public static void main(String[] args) throws Exception {
        if (args.length < 2) {
            System.err.println("usage: HotReloadAgent <socket> <ping|redefine-dir> [directory]");
            System.exit(2);
        }

        String command =
            switch (args[1]) {
                case "ping" -> "PING";
                case "redefine-dir" -> {
                    if (args.length != 3) {
                        System.err.println("redefine-dir requires a directory");
                        System.exit(2);
                    }
                    yield "REDEFINE_DIR " + args[2];
                }
                default -> {
                    System.err.println("unknown command: " + args[1]);
                    System.exit(2);
                    yield "";
                }
            };

        try (SocketChannel client = SocketChannel.open(StandardProtocolFamily.UNIX)) {
            client.connect(UnixDomainSocketAddress.of(Path.of(args[0])));
            BufferedWriter writer =
                new BufferedWriter(new OutputStreamWriter(Channels.newOutputStream(client), StandardCharsets.UTF_8));
            BufferedReader reader =
                new BufferedReader(new InputStreamReader(Channels.newInputStream(client), StandardCharsets.UTF_8));
            writer.write(command);
            writer.newLine();
            writer.flush();

            String response = reader.readLine();
            if (response == null) {
                System.err.println("no response from hot reload agent");
                System.exit(1);
            }
            System.out.println(response);
            if (!response.startsWith("OK ")) {
                System.exit(1);
            }
        }
    }
}
