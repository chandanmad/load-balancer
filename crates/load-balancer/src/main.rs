use std::sync::Arc;

use load_balancer::configuration::ServerConfig;
use load_balancer::metric::Metrics;
use load_balancer::server::Server;
use load_balancer::throttle::DummyRatelimit;
use pingora::server::configuration::Opt;

fn main() {
    // Enable basic logging; set RUST_LOG=info for visibility.
    env_logger::init();

    // Read command line arguments
    let opt = Opt::parse_args();

    // Create new Server wrapper
    let mut server = Server::new(Some(opt)).expect("Failed to create server");

    // We need to read the configuration file (passing the path if provided in Opt, but Opt might not expose the path directly in a way we can re-read easily if we want "our" fields)
    // Pingora's Server::new loads the config into server.configuration.
    // However, Pingora's ServerConf is unrelated to our ServerConfig struct.
    // We assumed we have a single file with both.
    // If we use Server::new(Some(opt)), Pingora reads the config file specified in -c/--conf.
    // We need to read that SAME file to get our `backend` field.

    // Hack: Get the config path from args again or assume it was passed.
    // Opt struct has `conf: Option<String>`.
    let conf_path = Opt::parse_args()
        .conf
        .unwrap_or_else(|| "conf.yaml".to_string());

    // Parse our part of the config
    let conf_str = std::fs::read_to_string(&conf_path).expect("Failed to read config file");
    let server_conf: ServerConfig =
        serde_yaml::from_str(&conf_str).expect("Failed to parse server config");

    let conf_path_buf = std::path::Path::new(&conf_path);
    let config_base_path = conf_path_buf.parent().unwrap_or(std::path::Path::new("."));

    server
        .bootstrap(
            server_conf,
            config_base_path,
            "0.0.0.0:8080",
            Arc::new(DummyRatelimit),
            Arc::new(Metrics::default()),
        )
        .expect("Failed to bootstrap server");

    server.run_forever();
}
