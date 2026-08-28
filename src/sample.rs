//! Sample data used to populate the UI before real log sources (files on disk,
//! Elasticsearch connections, ...) are wired up.

/// A single log "document" shown in a tab.
pub struct LogFile {
    pub name: String,
    pub path: String,
    pub lines: Vec<String>,
}

impl LogFile {
    /// Rough on-disk size, derived from the sample contents.
    pub fn size_kib(&self) -> f64 {
        let bytes: usize = self.lines.iter().map(|l| l.len() + 1).sum();
        bytes as f64 / 1024.0
    }
}

/// A node in the left-hand file picker tree.
pub enum PickerNode {
    Folder {
        name: String,
        children: Vec<PickerNode>,
    },
    File {
        name: String,
        file: usize,
    },
}

fn folder(name: &str, children: Vec<PickerNode>) -> PickerNode {
    PickerNode::Folder {
        name: name.to_string(),
        children,
    }
}

fn file(name: &str, file: usize) -> PickerNode {
    PickerNode::File {
        name: name.to_string(),
        file,
    }
}

/// Builds the sample files together with the tree that references them by index.
pub fn library() -> (Vec<LogFile>, Vec<PickerNode>) {
    let files = vec![
        nginx_access(),
        nginx_error(),
        application_log(),
        syslog(),
        build_log(),
    ];

    let tree = vec![folder(
        "Sample Logs",
        vec![
            folder(
                "nginx",
                vec![file("access.log", 0), file("error.log", 1)],
            ),
            file("application.log", 2),
            file("syslog", 3),
            file("build.log", 4),
        ],
    )];

    (files, tree)
}

fn lines(raw: &str) -> Vec<String> {
    raw.trim_matches('\n').lines().map(str::to_string).collect()
}

fn nginx_access() -> LogFile {
    let paths = [
        ("GET", "/", 200, 612),
        ("GET", "/assets/app.css", 200, 18422),
        ("GET", "/assets/app.js", 200, 90211),
        ("GET", "/api/session", 200, 172),
        ("GET", "/gallery", 200, 2309),
        ("GET", "/favicon.ico", 404, 181),
        ("POST", "/api/login", 200, 88),
        ("GET", "/api/user/profile", 200, 431),
        ("GET", "/images/hero.jpg", 200, 386480),
        ("GET", "/api/orders?page=2", 200, 5122),
        ("POST", "/api/checkout", 500, 640),
        ("GET", "/api/orders?page=3", 200, 4980),
        ("GET", "/health", 200, 2),
        ("DELETE", "/api/cart/17", 204, 0),
        ("GET", "/api/search?q=lens", 200, 7810),
        ("GET", "/static/fonts/inter.woff2", 200, 76372),
        ("GET", "/admin", 403, 149),
        ("GET", "/api/metrics", 200, 1204),
        ("PUT", "/api/user/settings", 200, 233),
        ("GET", "/robots.txt", 200, 68),
    ];

    let mut out = Vec::new();
    let mut second = 1;
    for i in 0..60 {
        let (method, path, status, bytes) = paths[i % paths.len()];
        let ip = format!("172.19.0.{}", (i % 6) + 1);
        let minute = 2 + i / 20;
        out.push(format!(
            "{ip} - - [07/Oct/2020:23:{:02}:{:02} +0300] \"{method} {path} HTTP/1.1\" {status} {bytes} \"http://127.0.0.1:8080/\" \"Mozilla/5.0 (X11; Linux x86_64)\"",
            minute,
            second,
        ));
        second = (second + 3) % 60;
    }
    LogFile {
        name: "access.log".to_string(),
        path: "/tmp/loglens_sample/logs/nginx/access.log".to_string(),
        lines: out,
    }
}

fn nginx_error() -> LogFile {
    LogFile {
        name: "error.log".to_string(),
        path: "/tmp/loglens_sample/logs/nginx/error.log".to_string(),
        lines: lines(
            r#"
2020/10/07 23:03:11 [notice] 1#1: using the "epoll" event method
2020/10/07 23:03:11 [notice] 1#1: nginx/1.19.2
2020/10/07 23:03:11 [notice] 1#1: OS: Linux 5.4.0-48-generic
2020/10/07 23:03:11 [notice] 1#1: start worker processes
2020/10/07 23:04:52 [error] 31#31: *5 open() "/var/www/html/favicon.ico" failed (2: No such file or directory), client: 172.19.0.1, server: localhost, request: "GET /favicon.ico HTTP/1.1"
2020/10/07 23:07:19 [warn] 31#31: *12 an upstream response is buffered to a temporary file /var/cache/nginx/proxy_temp/3/00/0000000003 while reading upstream
2020/10/07 23:09:40 [error] 31#31: *18 connect() failed (111: Connection refused) while connecting to upstream, client: 172.19.0.3, server: localhost, request: "POST /api/checkout HTTP/1.1", upstream: "http://127.0.0.1:9000/api/checkout"
2020/10/07 23:09:40 [error] 31#31: *18 upstream sent no valid HTTP/1.0 header while reading response header from upstream
2020/10/07 23:11:02 [crit] 31#31: *24 SSL_do_handshake() failed (SSL: error:141CF06C:SSL routines:tls_parse_ctos_key_share:bad key share)
2020/10/07 23:12:33 [warn] 31#31: *31 client intended to send too large body: 2100000 bytes
2020/10/07 23:14:05 [notice] 1#1: signal 1 (SIGHUP) received from 42, reconfiguring
2020/10/07 23:14:05 [notice] 1#1: reloading configuration
"#,
        ),
    }
}

fn application_log() -> LogFile {
    LogFile {
        name: "application.log".to_string(),
        path: "/tmp/loglens_sample/logs/application.log".to_string(),
        lines: lines(
            r#"
2026-08-28 09:40:01.204 INFO  [main] c.l.LogLensApplication - Starting LogLensApplication v0.1.0
2026-08-28 09:40:01.780 INFO  [main] c.l.config.DataSourceConfig - HikariPool-1 - Starting...
2026-08-28 09:40:02.113 INFO  [main] c.l.config.DataSourceConfig - HikariPool-1 - Start completed
2026-08-28 09:40:02.640 DEBUG [main] c.l.index.IndexRegistry - Registered 3 index templates
2026-08-28 09:40:03.001 INFO  [main] c.l.LogLensApplication - Started LogLensApplication in 2.31 seconds
2026-08-28 09:40:12.556 INFO  [http-nio-8080-exec-1] c.l.web.SearchController - query="status:500" range=15m hits=4
2026-08-28 09:40:44.982 WARN  [pool-2-thread-1] c.l.tail.FileTailer - Detected truncation on /var/log/app/worker.log, reopening
2026-08-28 09:41:03.417 ERROR [http-nio-8080-exec-4] c.l.web.SearchController - Failed to parse query expression 'level:('
java.lang.IllegalStateException: Unbalanced parenthesis at position 6
	at c.l.query.QueryParser.expect(QueryParser.java:184)
	at c.l.query.QueryParser.parse(QueryParser.java:72)
	at c.l.web.SearchController.search(SearchController.java:91)
2026-08-28 09:41:05.900 INFO  [http-nio-8080-exec-5] c.l.web.SearchController - query="service:checkout AND level:ERROR" range=1h hits=27
2026-08-28 09:41:18.245 WARN  [scheduler-1] c.l.retention.RetentionJob - Index loglens-2026.07.29 exceeds retention (30d), scheduling delete
2026-08-28 09:41:29.671 INFO  [scheduler-1] c.l.retention.RetentionJob - Deleted 1 index, reclaimed 812 MB
2026-08-28 09:41:41.008 DEBUG [http-nio-8080-exec-6] c.l.web.SearchController - cache hit for saved search 'checkout-errors'
"#,
        ),
    }
}

fn syslog() -> LogFile {
    LogFile {
        name: "syslog".to_string(),
        path: "/tmp/loglens_sample/logs/syslog".to_string(),
        lines: lines(
            r#"
Aug 28 09:30:01 workbench CRON[20413]: (root) CMD (   cd / && run-parts --report /etc/cron.hourly)
Aug 28 09:31:14 workbench systemd[1]: Starting Daily apt download activities...
Aug 28 09:31:16 workbench systemd[1]: apt-daily.service: Succeeded.
Aug 28 09:31:16 workbench systemd[1]: Finished Daily apt download activities.
Aug 28 09:33:42 workbench kernel: [128841.552013] wlp3s0: authenticate with 3c:37:86:1f:aa:20
Aug 28 09:33:42 workbench kernel: [128841.598111] wlp3s0: associated
Aug 28 09:34:05 workbench NetworkManager[912]: <info>  [1756373645.0021] dhcp4 (wlp3s0): state changed new lease, address=192.168.1.114
Aug 28 09:35:19 workbench gnome-shell[1841]: Window manager warning: last_user_time (2371841) is greater than comparison timestamp
Aug 28 09:36:02 workbench sudo[20777]: dschimtzek : TTY=pts/2 ; PWD=/home/dschimtzek ; USER=root ; COMMAND=/usr/bin/systemctl restart docker
Aug 28 09:36:02 workbench systemd[1]: Stopping Docker Application Container Engine...
Aug 28 09:36:04 workbench systemd[1]: docker.service: Succeeded.
Aug 28 09:36:05 workbench systemd[1]: Started Docker Application Container Engine.
Aug 28 09:37:33 workbench kernel: [129172.004881] Out of memory: Killed process 21044 (node) total-vm:4210112kB
"#,
        ),
    }
}

fn build_log() -> LogFile {
    LogFile {
        name: "build.log".to_string(),
        path: "/tmp/loglens_sample/logs/build.log".to_string(),
        lines: lines(
            r#"
   Compiling proc-macro2 v1.0.86
   Compiling unicode-ident v1.0.12
   Compiling libc v0.2.155
   Compiling cfg-if v1.0.0
   Compiling autocfg v1.3.0
   Compiling iced_core v0.14.0
   Compiling iced_widget v0.14.2
   Compiling loglens v0.1.0 (/home/dschimtzek/ghq/github.com/dennisdms/loglens)
warning: unused import: `iced::Font`
 --> src/main.rs:2:5
warning: `loglens` (bin "loglens") generated 1 warning
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.44s
     Running `target/debug/loglens`
"#,
        ),
    }
}
