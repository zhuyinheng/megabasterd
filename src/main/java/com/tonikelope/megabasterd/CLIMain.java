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
import java.io.BufferedOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.InetSocketAddress;
import java.net.Proxy;
import java.net.URL;
import java.sql.SQLException;
import java.util.HashMap;
import java.util.Map;
import java.util.logging.Level;
import java.util.logging.Logger;
import javax.crypto.CipherInputStream;

/**
 * CLI mode entry point for MegaBasterd.
 *
 * Usage:
 *   java -jar MegaBasterd.jar --cli --url <mega-link> --output <dir>
 *                             [--email <email> --password <password>]
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
        String url         = null;
        String outputDir   = null;
        String email       = null;
        String password    = null;
        String proxyHost   = null;
        int    proxyPort   = 8080;
        String proxyUser   = null;
        String proxyPass   = null;

        for (int i = 0; i < args.length; i++) {
            switch (args[i]) {
                case "--url":    case "-u": if (i + 1 < args.length) url       = args[++i]; break;
                case "--output": case "-o": if (i + 1 < args.length) outputDir  = args[++i]; break;
                case "--email":  case "-e": if (i + 1 < args.length) email      = args[++i]; break;
                case "--password": case "-p": if (i + 1 < args.length) password = args[++i]; break;
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
                downloadFolder(api, url, outputDir, proxyHost, proxyPort, proxyUser, proxyPass);
            } else {
                downloadUrl(api, url, outputDir, proxyHost, proxyPort, proxyUser, proxyPass);
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
            String proxyHost, int proxyPort, String proxyUser, String proxyPass)
            throws Exception {

        // #F!FOLDER_ID!FOLDER_KEY
        String folderId  = findFirstRegex("#F!([^!@]+)", legacyUrl, 1);
        String folderKey = findFirstRegex("#F![^!]+!([^!\\s]+)", legacyUrl, 1);

        if (folderId == null || folderKey == null) {
            throw new Exception("Could not parse folder ID/key from: " + legacyUrl);
        }

        System.out.println("[INFO] Fetching folder file tree for folder: " + folderId);
        HashMap<String, Object> nodes = api.getFolderNodes(folderId, folderKey, null, false);

        // count files
        long totalSize = 0;
        int fileCount = 0;
        for (Object val : nodes.values()) {
            HashMap<String, Object> node = (HashMap<String, Object>) val;
            Integer type = (Integer) node.get("type");
            if (type != null && type == 0) {
                fileCount++;
                Object sz = node.get("size");
                if (sz instanceof Long) totalSize += (Long) sz;
            }
        }

        System.out.printf("[INFO] Found %d file(s) (%s total) in folder.%n", fileCount, formatBytes(totalSize));

        int idx = 0;
        for (Map.Entry<String, Object> entry : nodes.entrySet()) {
            HashMap<String, Object> node = (HashMap<String, Object>) entry.getValue();
            Integer type = (Integer) node.get("type");
            if (type == null || type != 0) continue;

            String nodeH   = (String) node.get("h");
            String nodeKey = (String) node.get("key");
            // Construct a node link that getMegaFileMetadata / getMegaFileDownloadUrl understand
            String nLink = "https://mega.nz/#N!" + nodeH + "!" + nodeKey + "###n=" + folderId;

            idx++;
            System.out.printf("%n[INFO] (%d/%d) %s%n", idx, fileCount, node.get("name"));

            downloadUrl(api, nLink, outputDir, proxyHost, proxyPort, proxyUser, proxyPass);
        }

        System.out.printf("%n[INFO] All %d file(s) downloaded to: %s%n", fileCount, new File(outputDir).getAbsolutePath());
    }

    // ── single-file download ──────────────────────────────────────────────────

    private static void downloadUrl(MegaAPI api, String url, String outputDir,
            String proxyHost, int proxyPort, String proxyUser, String proxyPass)
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

        // 5. chunk-based download with console progress
        long bytesDownloaded = 0;
        long chunkId = 1;

        try (BufferedOutputStream bos = new BufferedOutputStream(new FileOutputStream(outFile))) {

            while (bytesDownloaded < fileSize) {

                long chunkOffset = ChunkWriterManager.calculateChunkOffset(chunkId, 1);

                // finished?
                if (chunkOffset >= fileSize) break;

                long chunkSize = ChunkWriterManager.calculateChunkSize(chunkId, fileSize, chunkOffset, 1);

                // download one chunk with retries
                bytesDownloaded = downloadChunk(
                        downloadUrl, chunkOffset, chunkSize,
                        byteKey, byteIV, bytesDownloaded,
                        bos, fileSize,
                        proxyHost, proxyPort, proxyUser, proxyPass);

                chunkId++;
            }

            bos.flush();
        }

        System.out.println("\n[INFO] Download complete: " + outFile.getAbsolutePath());
    }

    /**
     * Downloads a single chunk, retrying on transient errors.
     * Returns the updated total-bytes-downloaded counter.
     */
    private static long downloadChunk(
            String downloadUrl, long chunkOffset, long chunkSize,
            byte[] byteKey, byte[] byteIV, long bytesDownloaded,
            BufferedOutputStream bos, long fileSize,
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

                // create AES-CTR cipher positioned at the current stream offset
                javax.crypto.Cipher cipher = genDecrypter(
                        "AES", "AES/CTR/NoPadding",
                        byteKey, forwardMEGALinkKeyIV(byteIV, bytesDownloaded));

                byte[] buffer = new byte[MainPanel.DEFAULT_BYTE_BUFFER_SIZE];

                try (CipherInputStream cis = new CipherInputStream(con.getInputStream(), cipher)) {
                    int reads;
                    while (chunkReads < chunkSize
                            && (reads = cis.read(buffer, 0,
                                    (int) Math.min(chunkSize - chunkReads, buffer.length))) != -1) {
                        bos.write(buffer, 0, reads);
                        chunkReads += reads;
                        bytesDownloaded += reads;
                        printProgress(bytesDownloaded, fileSize);
                    }
                }

                // success
                return bytesDownloaded;

            } catch (IOException ex) {

                if (retries >= MAX_RETRIES) {
                    throw ex;
                }

                // roll back partial progress
                bytesDownloaded -= chunkReads;

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
        System.out.println("       [--proxy-host <host> --proxy-port <port>]");
        System.out.println("       [--proxy-user <user> --proxy-pass <pass>]");
        System.out.println();
        System.out.println("Options:");
        System.out.println("  -u, --url       MEGA link (file, folder, or file-within-folder link)");
        System.out.println("  -o, --output    Output directory (default: current directory)");
        System.out.println("  -e, --email     MEGA account e-mail (optional)");
        System.out.println("  -p, --password  MEGA account password (optional)");
        System.out.println("      --proxy-host  HTTP proxy host");
        System.out.println("      --proxy-port  HTTP proxy port (default: 8080)");
        System.out.println("      --proxy-user  Proxy username");
        System.out.println("      --proxy-pass  Proxy password");
        System.out.println("  -h, --help      Show this help message");
    }
}
