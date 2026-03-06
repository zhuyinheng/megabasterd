/*
 __  __                  _               _               _
|  \/  | ___  __ _  __ _| |__   __ _ ___| |_ ___ _ __ __| |
| |\/| |/ _ \/ _` |/ _` | '_ \ / _` / __| __/ _ \ '__/ _` |
| |  | |  __/ (_| | (_| | |_) | (_| \__ \ ||  __/ | | (_| |
|_|  |_|\___|\__, |\__,_|_.__/ \__,_|___/\__\___|_|  \__,_|
             |___/
© Perpetrated by tonikelope since 2016
 */
package com.tonikelope.megabasterd;

import static com.tonikelope.megabasterd.CryptTools.*;
import static com.tonikelope.megabasterd.MiscTools.*;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.InetSocketAddress;
import java.net.Proxy;
import java.net.URL;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;
import java.sql.SQLException;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.logging.Level;
import java.util.logging.Logger;
import javax.crypto.CipherInputStream;

/**
 * CLI mode entry point for MegaBasterd.
 *
 * Usage:
 *   java -jar MegaBasterd.jar --cli --url <mega-link> --output <dir>
 *                             [--email <email> --password <password>]
 *                             [--parallel <N>]  (max concurrent file downloads, default 3)
 *                             [--slots <N>]     (chunk slots per file, default 1)
 *                             [--proxy-host <host> --proxy-port <port>]
 *                             [--proxy-user <user> --proxy-pass <pass>]
 *
 * @author tonikelope
 */
public class CLIMain {

    private static final Logger LOG = Logger.getLogger(CLIMain.class.getName());

    private static final int CONNECT_TIMEOUT_MS = 30_000;
    private static final int READ_TIMEOUT_MS    = 30_000;
    private static final int MAX_RETRIES        = 5;

    public static void run(String[] args) {

        // ── parse arguments ─────────────────────────────────────────────────
        String url              = null;
        String outputDir        = null;
        String email            = null;
        String password         = null;
        String proxyHost        = null;
        int    proxyPort        = 8080;
        String proxyUser        = null;
        String proxyPass        = null;
        int    parallelDownloads = 3;
        int    slots            = 1;

        for (int i = 0; i < args.length; i++) {
            switch (args[i]) {
                case "--url":      case "-u": if (i + 1 < args.length) url          = args[++i]; break;
                case "--output":   case "-o": if (i + 1 < args.length) outputDir    = args[++i]; break;
                case "--email":    case "-e": if (i + 1 < args.length) email        = args[++i]; break;
                case "--password": case "-p": if (i + 1 < args.length) password     = args[++i]; break;
                case "--parallel": case "-n":
                    if (i + 1 < args.length) { try { parallelDownloads = Math.max(1, Integer.parseInt(args[++i])); } catch (NumberFormatException ignored) {} } break;
                case "--slots":    case "-s":
                    if (i + 1 < args.length) { try { slots = Math.max(1, Integer.parseInt(args[++i])); } catch (NumberFormatException ignored) {} } break;
                case "--proxy-host": if (i + 1 < args.length) proxyHost = args[++i]; break;
                case "--proxy-port": if (i + 1 < args.length) { try { proxyPort = Integer.parseInt(args[++i]); } catch (NumberFormatException ignored) {} } break;
                case "--proxy-user": if (i + 1 < args.length) proxyUser = args[++i]; break;
                case "--proxy-pass": if (i + 1 < args.length) proxyPass = args[++i]; break;
                case "--help": case "-h": printHelp(); return;
                default: break;
            }
        }

        if (url == null) {
            System.err.println("[ERROR] --url is required.");
            printHelp();
            System.exit(1);
        }

        if (outputDir == null) {
            outputDir = ".";
        }

        // ── normalise new-style MEGA URLs to legacy format ───────────────────
        // e.g. https://mega.nz/folder/ID#KEY  →  https://mega.nz/#F!ID!KEY
        //      https://mega.nz/file/ID#KEY    →  https://mega.nz/#!ID!KEY
        url = newMegaLinks2Legacy(url).trim();

        // ── initialise SQLite (loads saved proxy / API-key settings) ─────────
        try {
            DBTools.setupSqliteTables();
        } catch (SQLException ex) {
            LOG.log(Level.WARNING, "Could not initialise SQLite: {0}", ex.getMessage());
        }

        // ── load proxy from DB if not overridden on command line ─────────────
        if (proxyHost == null) {
            String useProxy = DBTools.selectSettingValue("use_proxy");
            if ("yes".equals(useProxy)) {
                proxyHost = DBTools.selectSettingValue("proxy_host");
                String pp = DBTools.selectSettingValue("proxy_port");
                if (pp != null && !pp.isEmpty()) {
                    try { proxyPort = Integer.parseInt(pp); } catch (NumberFormatException ignored) {}
                }
                proxyUser = DBTools.selectSettingValue("proxy_user");
                proxyPass = DBTools.selectSettingValue("proxy_pass");
            }
        }

        // propagate proxy settings into MainPanel static fields so MegaAPI
        // uses them for its HTTPS API calls
        if (proxyHost != null && !proxyHost.isEmpty()) {
            MainPanel.setProxySettings(proxyHost, proxyPort, proxyUser, proxyPass, true);
        }

        // load optional MEGA API key
        String apiKey = DBTools.selectSettingValue("mega_api_key");
        if (apiKey != null && !apiKey.trim().isEmpty()) {
            MegaAPI.API_KEY = apiKey.trim();
        }

        // ── create MegaAPI instance, optionally log in ───────────────────────
        MegaAPI api = new MegaAPI();

        if (email != null && password != null) {
            System.out.println("[INFO] Logging in as " + email + " ...");
            try {
                api.login(email, password, null);
                System.out.println("[INFO] Logged in successfully.");
            } catch (Exception ex) {
                System.err.println("[ERROR] Login failed: " + ex.getMessage());
                System.exit(1);
            }
        }

        // ── dispatch: folder vs single file ──────────────────────────────────
        try {
            if (findFirstRegex("#F!", url, 0) != null) {
                downloadFolder(api, url, outputDir, proxyHost, proxyPort, proxyUser, proxyPass,
                        parallelDownloads, slots);
            } else {
                downloadUrl(api, url, outputDir, proxyHost, proxyPort, proxyUser, proxyPass, slots);
            }
        } catch (Exception ex) {
            System.err.println("[ERROR] Download failed: " + ex.getMessage());
            LOG.log(Level.SEVERE, ex.getMessage(), ex);
            System.exit(1);
        }
    }

    // ── folder download ───────────────────────────────────────────────────────

    @SuppressWarnings("unchecked")
    private static void downloadFolder(MegaAPI api, String legacyUrl, String outputDir,
            String proxyHost, int proxyPort, String proxyUser, String proxyPass,
            int parallelDownloads, int slots)
            throws Exception {

        // #F!FOLDER_ID!FOLDER_KEY
        String folderId  = findFirstRegex("#F!([^!@]+)", legacyUrl, 1);
        String folderKey = findFirstRegex("#F![^!]+!([^!\\s]+)", legacyUrl, 1);

        if (folderId == null || folderKey == null) {
            throw new Exception("Could not parse folder ID/key from: " + legacyUrl);
        }

        System.out.println("[INFO] Fetching folder file tree for folder: " + folderId);
        HashMap<String, Object> nodes = api.getFolderNodes(folderId, folderKey, null, false);

        // collect file nodes
        List<HashMap<String, Object>> fileNodes = new ArrayList<>();
        long totalSize = 0;
        for (Object val : nodes.values()) {
            HashMap<String, Object> node = (HashMap<String, Object>) val;
            Integer type = (Integer) node.get("type");
            if (type != null && type == 0) {
                fileNodes.add(node);
                Object sz = node.get("size");
                if (sz instanceof Long) totalSize += (Long) sz;
            }
        }

        int fileCount = fileNodes.size();
        System.out.printf("[INFO] Found %d file(s) (%s total) — parallel=%d  slots=%d%n",
                fileCount, formatBytes(totalSize), parallelDownloads, slots);

        // ── parallel dispatch ────────────────────────────────────────────────
        final String finalFolderId  = folderId;
        final String finalProxyHost = proxyHost;
        final int    finalProxyPort = proxyPort;
        final String finalProxyUser = proxyUser;
        final String finalProxyPass = proxyPass;
        final int    finalSlots     = slots;

        AtomicLong completedCount = new AtomicLong(0);
        AtomicReference<Exception> firstError = new AtomicReference<>();

        ExecutorService pool = Executors.newFixedThreadPool(parallelDownloads);
        List<Future<?>> futures = new ArrayList<>();

        for (int i = 0; i < fileNodes.size(); i++) {
            HashMap<String, Object> node = fileNodes.get(i);
            String nodeH   = (String) node.get("h");
            String nodeKey = (String) node.get("key");
            String nLink   = "https://mega.nz/#N!" + nodeH + "!" + nodeKey + "###n=" + finalFolderId;
            String name    = (String) node.get("name");
            final int idx  = i + 1;

            futures.add(pool.submit(() -> {
                try {
                    System.out.printf("%n[INFO] (%d/%d) Starting: %s%n", idx, fileCount, name);
                    downloadUrl(api, nLink, outputDir,
                            finalProxyHost, finalProxyPort, finalProxyUser, finalProxyPass, finalSlots);
                    System.out.printf("%n[INFO] (%d/%d) Done: %s%n",
                            completedCount.incrementAndGet(), fileCount, name);
                } catch (Exception ex) {
                    firstError.compareAndSet(null, ex);
                    System.err.printf("%n[ERROR] (%d/%d) Failed: %s — %s%n",
                            idx, fileCount, name, ex.getMessage());
                }
            }));
        }

        pool.shutdown();
        for (Future<?> f : futures) {
            try { f.get(); } catch (Exception ignored) {}
        }

        if (firstError.get() != null) {
            throw firstError.get();
        }

        System.out.printf("%n[INFO] All %d file(s) downloaded to: %s%n",
                fileCount, new File(outputDir).getAbsolutePath());
    }

    // ── single-file download (dispatcher) ────────────────────────────────────

    private static void downloadUrl(MegaAPI api, String url, String outputDir,
            String proxyHost, int proxyPort, String proxyUser, String proxyPass,
            int slots)
            throws Exception {

        // 1. get file metadata
        System.out.println("[INFO] Fetching file metadata ...");
        String[] meta = api.getMegaFileMetadata(url);
        String fileName = meta[0];
        long   fileSize = Long.parseLong(meta[1]);
        String fileKey  = meta[2];

        System.out.printf("[INFO] File: %s  (%s)%n", fileName, formatBytes(fileSize));

        // 2. get download URL
        System.out.println("[INFO] Retrieving download URL ...");
        String downloadUrl = api.getMegaFileDownloadUrl(url);

        // 3. prepare output file
        File outDir = new File(outputDir);
        outDir.mkdirs();
        File outFile = new File(outDir, fileName);

        // avoid overwriting: append a suffix if a file already exists
        if (outFile.exists()) {
            String base = fileName.replaceFirst("\\.[^.]+$", "");
            String ext  = fileName.contains(".") ? fileName.substring(fileName.lastIndexOf('.')) : "";
            outFile = new File(outDir, base + "_" + genID(6) + ext);
        }

        System.out.println("[INFO] Saving to: " + outFile.getAbsolutePath());

        // 4. initialise AES-CTR key + IV
        byte[] byteKey = initMEGALinkKey(fileKey);
        byte[] byteIV  = initMEGALinkKeyIV(fileKey);

        // 5. dispatch to single-slot or multi-slot download
        if (slots <= 1) {
            downloadSingleSlot(downloadUrl, outFile, fileSize, byteKey, byteIV,
                    proxyHost, proxyPort, proxyUser, proxyPass);
        } else {
            downloadMultiSlot(downloadUrl, outFile, fileSize, byteKey, byteIV,
                    proxyHost, proxyPort, proxyUser, proxyPass, slots);
        }

        System.out.println("\n[INFO] Download complete: " + outFile.getAbsolutePath());
    }

    // ── single-slot sequential download ──────────────────────────────────────

    private static void downloadSingleSlot(String downloadUrl, File outFile, long fileSize,
            byte[] byteKey, byte[] byteIV,
            String proxyHost, int proxyPort, String proxyUser, String proxyPass)
            throws Exception {

        AtomicLong bytesDownloaded = new AtomicLong(0);
        long chunkId = 1;

        try (FileOutputStream fos = new FileOutputStream(outFile);
             FileChannel channel = fos.getChannel()) {

            while (bytesDownloaded.get() < fileSize) {

                long chunkOffset = ChunkWriterManager.calculateChunkOffset(chunkId, 1);
                if (chunkOffset >= fileSize) break;
                long chunkSize = ChunkWriterManager.calculateChunkSize(chunkId, fileSize, chunkOffset, 1);

                downloadChunkToChannel(downloadUrl, chunkOffset, chunkSize, byteKey, byteIV,
                        channel, bytesDownloaded, fileSize, proxyHost, proxyPort, proxyUser, proxyPass);

                chunkId++;
            }
        }
    }

    // ── multi-slot parallel download ──────────────────────────────────────────

    private static void downloadMultiSlot(String downloadUrl, File outFile, long fileSize,
            byte[] byteKey, byte[] byteIV,
            String proxyHost, int proxyPort, String proxyUser, String proxyPass,
            int slots)
            throws Exception {

        // 1. pre-calculate all chunks and put them in a work queue
        ConcurrentLinkedQueue<long[]> queue = new ConcurrentLinkedQueue<>(); // {chunkId, offset, size}
        for (long chunkId = 1; ; chunkId++) {
            long offset = ChunkWriterManager.calculateChunkOffset(chunkId, 1);
            if (offset >= fileSize) break;
            long size = ChunkWriterManager.calculateChunkSize(chunkId, fileSize, offset, 1);
            queue.offer(new long[]{chunkId, offset, size});
        }

        AtomicLong totalDownloaded = new AtomicLong(0);
        AtomicReference<Exception> firstError = new AtomicReference<>();

        // 2. open the output file once; FileChannel.write(buf, pos) is thread-safe
        try (FileOutputStream fos = new FileOutputStream(outFile);
             FileChannel channel = fos.getChannel()) {

            // pre-allocate so seek-writes work correctly
            channel.write(ByteBuffer.allocate(1), fileSize - 1);
            channel.force(false);

            // 3. spawn slot threads
            ExecutorService slotPool = Executors.newFixedThreadPool(slots);
            List<Future<?>> futures = new ArrayList<>();

            for (int s = 0; s < slots; s++) {
                futures.add(slotPool.submit(() -> {
                    long[] chunk;
                    while ((chunk = queue.poll()) != null && firstError.get() == null) {
                        try {
                            downloadChunkToChannel(downloadUrl, chunk[1], chunk[2],
                                    byteKey, byteIV, channel,
                                    totalDownloaded, fileSize,
                                    proxyHost, proxyPort, proxyUser, proxyPass);
                        } catch (Exception ex) {
                            firstError.compareAndSet(null, ex);
                        }
                    }
                }));
            }

            slotPool.shutdown();
            for (Future<?> f : futures) {
                try { f.get(); } catch (Exception ignored) {}
            }
        }

        if (firstError.get() != null) {
            throw firstError.get();
        }
    }

    // ── chunk download → FileChannel ─────────────────────────────────────────

    private static void downloadChunkToChannel(
            String downloadUrl, long chunkOffset, long chunkSize,
            byte[] byteKey, byte[] byteIV,
            FileChannel channel,
            AtomicLong totalDownloaded, long fileSize,
            String proxyHost, int proxyPort, String proxyUser, String proxyPass)
            throws Exception {

        int retries = 0;

        while (true) {
            HttpURLConnection con = null;
            long chunkReads = 0;

            try {
                URL chunkUrl = new URL(downloadUrl + "/" + chunkOffset);

                if (proxyHost != null && !proxyHost.isEmpty()) {
                    Proxy proxy = new Proxy(Proxy.Type.HTTP, new InetSocketAddress(proxyHost, proxyPort));
                    con = (HttpURLConnection) chunkUrl.openConnection(proxy);
                    if (proxyUser != null && !proxyUser.isEmpty()) {
                        con.setRequestProperty("Proxy-Authorization",
                                "Basic " + Bin2BASE64((proxyUser + ":" + proxyPass).getBytes("UTF-8")));
                    }
                } else {
                    con = (HttpURLConnection) chunkUrl.openConnection();
                }

                con.setConnectTimeout(CONNECT_TIMEOUT_MS);
                con.setReadTimeout(READ_TIMEOUT_MS);
                con.setUseCaches(false);
                con.setRequestProperty("User-Agent", MainPanel.DEFAULT_USER_AGENT);

                int httpStatus = con.getResponseCode();
                if (httpStatus != 200) {
                    throw new IOException("HTTP " + httpStatus + " for chunk at offset " + chunkOffset);
                }

                // AES-CTR cipher positioned at the correct stream offset
                javax.crypto.Cipher cipher = genDecrypter(
                        "AES", "AES/CTR/NoPadding",
                        byteKey, forwardMEGALinkKeyIV(byteIV, chunkOffset));

                byte[] buffer = new byte[MainPanel.DEFAULT_BYTE_BUFFER_SIZE];

                try (CipherInputStream cis = new CipherInputStream(con.getInputStream(), cipher)) {
                    int reads;
                    while (chunkReads < chunkSize
                            && (reads = cis.read(buffer, 0,
                                    (int) Math.min(chunkSize - chunkReads, buffer.length))) != -1) {

                        ByteBuffer bb = ByteBuffer.wrap(buffer, 0, reads);
                        // positional write — thread-safe on FileChannel
                        long pos = chunkOffset + chunkReads;
                        while (bb.hasRemaining()) {
                            channel.write(bb, pos + (reads - bb.remaining()));
                        }
                        chunkReads += reads;
                        printProgress(totalDownloaded.addAndGet(reads), fileSize);
                    }
                }

                return; // success

            } catch (IOException ex) {

                if (retries >= MAX_RETRIES) throw ex;

                totalDownloaded.addAndGet(-chunkReads);
                chunkReads = 0;

                long waitSecs = getWaitTimeExpBackOff(++retries);
                System.err.printf("%n[WARN] Chunk error (%s), retrying in %ds (attempt %d/%d)...%n",
                        ex.getMessage(), waitSecs, retries, MAX_RETRIES);
                try { Thread.sleep(waitSecs * 1000); } catch (InterruptedException ie) { Thread.currentThread().interrupt(); }

            } finally {
                if (con != null) con.disconnect();
            }
        }
    }

    private static void printProgress(long downloaded, long total) {
        if (total <= 0) return;
        int pct = (int) (downloaded * 100L / total);
        int barLen = 40;
        int filled = pct * barLen / 100;
        StringBuilder bar = new StringBuilder("[");
        for (int i = 0; i < barLen; i++) bar.append(i < filled ? '=' : ' ');
        bar.append(']');
        System.out.printf("\r%s %3d%%  %s / %s   ",
                bar, pct, formatBytes(downloaded), formatBytes(total));
        System.out.flush();
    }

    private static void printHelp() {
        System.out.println("MegaBasterd CLI mode");
        System.out.println();
        System.out.println("Usage:");
        System.out.println("  java -jar MegaBasterd.jar --cli --url <mega-link> --output <dir>");
        System.out.println("       [--email <email> --password <password>]");
        System.out.println("       [--parallel <N>]  [--slots <N>]");
        System.out.println("       [--proxy-host <host> --proxy-port <port>]");
        System.out.println("       [--proxy-user <user> --proxy-pass <pass>]");
        System.out.println();
        System.out.println("Options:");
        System.out.println("  -u, --url        MEGA link (file, folder, or file-within-folder link)");
        System.out.println("  -o, --output     Output directory (default: current directory)");
        System.out.println("  -e, --email      MEGA account e-mail (optional)");
        System.out.println("  -p, --password   MEGA account password (optional)");
        System.out.println("  -n, --parallel   Max concurrent file downloads for folder links (default: 3)");
        System.out.println("  -s, --slots      Parallel chunk connections per file (default: 1)");
        System.out.println("      --proxy-host  HTTP proxy host");
        System.out.println("      --proxy-port  HTTP proxy port (default: 8080)");
        System.out.println("      --proxy-user  Proxy username");
        System.out.println("      --proxy-pass  Proxy password");
        System.out.println("  -h, --help       Show this help message");
    }
}
