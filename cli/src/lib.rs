use anyhow::{Error, Result};
use clap::{Parser, Subcommand};
use log::info;
use ontoenv::api::{OntoEnv, ResolveTarget};
use ontoenv::config::Config;
use ontoenv::ontology::{GraphIdentifier, OntologyLocation};
use ontoenv::options::{Overwrite, RefreshStrategy};
use ontoenv::util::write_dataset_to_file;
use ontoenv::ToUriString;
use oxigraph::io::{JsonLdProfileSet, RdfFormat};
use oxigraph::model::NamedNode;
use std::collections::{BTreeMap, BTreeSet};
use std::env::current_dir;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "ontoenv")]
#[command(about = "Ontology environment manager")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Verbose mode - sets the RUST_LOG level to info, defaults to warning level
    #[clap(long, short, action, default_value = "false", global = true)]
    verbose: bool,
    /// Debug mode - sets the RUST_LOG level to debug, defaults to warning level
    #[clap(long, action, default_value = "false", global = true)]
    debug: bool,
    /// Resolution policy for determining which ontology to use when there are multiple with the same name
    #[clap(long, short, default_value = "default", global = true)]
    policy: Option<String>,
    /// Temporary (non-persistent) mode - will not save the environment to disk
    #[clap(long, short, action, global = true)]
    temporary: bool,
    /// Require ontology names to be unique; will raise an error if multiple ontologies have the same name
    #[clap(long, action, global = true)]
    require_ontology_names: bool,
    /// Strict mode - will raise an error if an ontology is not found
    #[clap(long, action, default_value = "false", global = true)]
    strict: bool,
    /// Offline mode - will not attempt to fetch ontologies from the web
    #[clap(long, short, action, default_value = "false", global = true)]
    offline: bool,
    /// Glob patterns for which files to include, defaults to ['*.ttl','*.xml','*.n3']
    #[clap(long, short, num_args = 1.., global = true)]
    includes: Vec<String>,
    /// Glob patterns for which files to exclude, defaults to []
    #[clap(long, short, num_args = 1.., global = true)]
    excludes: Vec<String>,
    /// Do not search for ontologies in the search directories
    #[clap(long = "no-search", short = 'n', action, global = true)]
    no_search: bool,
}

#[derive(Debug, Subcommand)]
enum ConfigCommands {
    /// Set a configuration value.
    Set {
        /// The configuration key to set.
        key: String,
        /// The value to set for the key.
        value: String,
    },
    /// Get a configuration value.
    Get {
        /// The configuration key to get.
        key: String,
    },
    /// Unset a configuration value, reverting to its default.
    Unset {
        /// The configuration key to unset.
        key: String,
    },
    /// Add a value to a list-based configuration key.
    Add {
        /// The configuration key to add to.
        key: String,
        /// The value to add.
        value: String,
    },
    /// Remove a value from a list-based configuration key.
    Remove {
        /// The configuration key to remove from.
        key: String,
        /// The value to remove.
        value: String,
    },
    /// List all configuration values.
    List,
}

#[derive(Debug, Subcommand)]
enum ListCommands {
    /// List all ontology locations found in the search paths
    Locations,
    /// List all declared ontologies in the environment
    Ontologies,
    /// List all missing imports
    Missing,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create a new ontology environment
    Init {
        /// Overwrite the environment if it already exists
        #[clap(long, default_value = "false")]
        overwrite: bool,
        /// Directories to search for ontologies. If not provided, the current directory is used.
        #[clap(last = true)]
        locations: Option<Vec<PathBuf>>,
    },
    /// Prints the version of the ontoenv binary
    Version,
    /// Prints the status of the ontology environment
    Status {
        /// Output JSON instead of text
        #[clap(long, action, default_value = "false")]
        json: bool,
    },
    /// Update the ontology environment
    Update {
        /// Suppress per-ontology update output
        #[clap(long, short = 'q', action)]
        quiet: bool,
        /// Update all ontologies, ignoring modification times
        #[clap(long, short = 'a', action)]
        all: bool,
        /// Output JSON instead of text
        #[clap(long, action, default_value = "false")]
        json: bool,
    },
    /// Compute the owl:imports closure of an ontology and write it to a file
    Closure {
        /// The name (URI) of the ontology to compute the closure for
        ontology: String,
        /// Do NOT rewrite sh:prefixes (rewrite is ON by default)
        #[clap(long, action, default_value = "false")]
        no_rewrite_sh_prefixes: bool,
        /// Keep owl:imports statements (removal is ON by default)
        #[clap(long, action, default_value = "false")]
        keep_owl_imports: bool,
        /// The file to write the closure to, defaults to 'output.ttl'
        destination: Option<String>,
        /// The recursion depth for exploring owl:imports. <0: unlimited, 0: no imports, >0:
        /// specific depth.
        #[clap(long, default_value = "-1")]
        recursion_depth: i32,
    },
    /// Retrieve a single graph from the environment and write it to STDOUT or a file
    Get {
        /// Ontology IRI (name)
        ontology: String,
        /// Optional source location (file path or URL) to disambiguate
        #[clap(long, short = 'l')]
        location: Option<String>,
        /// Output file path; if omitted, writes to STDOUT
        #[clap(long)]
        output: Option<String>,
        /// Serialization format: one of [turtle, ntriples, rdfxml, jsonld] (default: turtle)
        #[clap(long, short = 'f')]
        format: Option<String>,
    },
    /// Add an ontology to the environment
    Add {
        /// The location of the ontology to add (file path or URL)
        location: String,
        /// Do not explore owl:imports of the added ontology
        #[clap(long, action)]
        no_imports: bool,
    },
    /// List various properties of the environment
    /// List various properties of the environment
    List {
        #[command(subcommand)]
        list_cmd: ListCommands,
        /// Output JSON instead of text
        #[clap(long, action, default_value = "false")]
        json: bool,
    },
    // TODO: dump all ontologies; nest by ontology name (sorted), w/n each ontology name list all
    // the places where that graph can be found. List basic stats: the metadata field in the
    // Ontology struct and # of triples in the graph; last updated; etc
    /// Print out the current state of the ontology environment
    Dump {
        /// Filter the output to only include ontologies that contain the given string in their
        /// name. Leave empty to include all ontologies.
        contains: Option<String>,
    },
    /// Generate a PDF of the dependency graph
    DepGraph {
        /// The root ontologies to start the graph from. Given by name (URI)
        roots: Option<Vec<String>>,
        /// The output file to write the PDF to, defaults to 'dep_graph.pdf'
        #[clap(long, short)]
        output: Option<String>,
    },
    /// Lists which ontologies import the given ontology
    Why {
        /// The name (URI) of the ontology to find importers for
        ontologies: Vec<String>,
        /// Output JSON instead of text
        #[clap(long, action, default_value = "false")]
        json: bool,
    },
    /// Run the doctor to check the environment for issues
    Doctor {
        /// Output JSON instead of text
        #[clap(long, action, default_value = "false")]
        json: bool,
    },
    /// Reset the ontology environment by removing the .ontoenv directory
    Reset {
        #[clap(long, short, action = clap::ArgAction::SetTrue, default_value = "false")]
        force: bool,
    },
    /// Manage ontoenv configuration.
    #[command(subcommand)]
    Config(ConfigCommands),
}

impl ToString for Commands {
    fn to_string(&self) -> String {
        match self {
            Commands::Init { .. } => "Init".to_string(),
            Commands::Version => "Version".to_string(),
            Commands::Status { .. } => "Status".to_string(),
            Commands::Update { .. } => "Update".to_string(),
            Commands::Closure { .. } => "Closure".to_string(),
            Commands::Get { .. } => "Get".to_string(),
            Commands::Add { .. } => "Add".to_string(),
            Commands::List { .. } => "List".to_string(),
            Commands::Dump { .. } => "Dump".to_string(),
            Commands::DepGraph { .. } => "DepGraph".to_string(),
            Commands::Why { .. } => "Why".to_string(),
            Commands::Doctor { .. } => "Doctor".to_string(),
            Commands::Reset { .. } => "Reset".to_string(),
            Commands::Config { .. } => "Config".to_string(),
        }
    }
}

fn handle_config_command(config_cmd: ConfigCommands, temporary: bool) -> Result<()> {
    if temporary {
        return Err(anyhow::anyhow!("Cannot manage config in temporary mode."));
    }
    let root = ontoenv::api::find_ontoenv_root()
        .ok_or_else(|| anyhow::anyhow!("Not in an ontoenv. Use `ontoenv init` to create one."))?;
    let config_path = root.join(".ontoenv").join("ontoenv.json");
    if !config_path.exists() {
        return Err(anyhow::anyhow!(
            "No ontoenv.json found. Use `ontoenv init`."
        ));
    }

    match config_cmd {
        ConfigCommands::List => {
            let config_str = std::fs::read_to_string(&config_path)?;
            let config_json: serde_json::Value = serde_json::from_str(&config_str)?;
            let pretty_json = serde_json::to_string_pretty(&config_json)?;
            println!("{}", pretty_json);
            return Ok(());
        }
        ConfigCommands::Get { ref key } => {
            let config_str = std::fs::read_to_string(&config_path)?;
            let config_json: serde_json::Value = serde_json::from_str(&config_str)?;
            let object = config_json
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("Invalid config format: not a JSON object."))?;

            if let Some(value) = object.get(key) {
                if let Some(s) = value.as_str() {
                    println!("{}", s);
                } else if let Some(arr) = value.as_array() {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            println!("{}", s);
                        } else {
                            println!("{}", item);
                        }
                    }
                } else {
                    println!("{}", value);
                }
            } else {
                println!("Configuration key '{}' not set.", key);
            }
            return Ok(());
        }
        _ => {}
    }

    // Modifying commands continue here.
    let config_str = std::fs::read_to_string(&config_path)?;
    let mut config_json: serde_json::Value = serde_json::from_str(&config_str)?;

    let object = config_json
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid config format: not a JSON object."))?;

    match config_cmd {
        ConfigCommands::Set { key, value } => {
            match key.as_str() {
                "offline" | "strict" | "require_ontology_names" | "no_search" => {
                    let bool_val = value.parse::<bool>().map_err(|_| {
                        anyhow::anyhow!("Invalid boolean value for {}: {}", key, value)
                    })?;
                    object.insert(key.to_string(), serde_json::Value::Bool(bool_val));
                }
                "resolution_policy" => {
                    object.insert(key.to_string(), serde_json::Value::String(value.clone()));
                }
                "locations" | "includes" | "excludes" => {
                    return Err(anyhow::anyhow!(
                        "Use `ontoenv config add/remove {} <value>` to modify list values.",
                        key
                    ));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Setting configuration for '{}' is not supported.",
                        key
                    ));
                }
            }
            println!("Set {} to {}", key, value);
        }
        ConfigCommands::Unset { key } => {
            if object.remove(&key).is_some() {
                println!("Unset '{}'.", key);
            } else {
                return Err(anyhow::anyhow!("Configuration key '{}' not set.", key));
            }
        }
        ConfigCommands::Add { key, value } => {
            match key.as_str() {
                "locations" | "includes" | "excludes" => {
                    let entry = object
                        .entry(key.clone())
                        .or_insert_with(|| serde_json::Value::Array(vec![]));
                    if let Some(arr) = entry.as_array_mut() {
                        let new_val = serde_json::Value::String(value.clone());
                        if !arr.contains(&new_val) {
                            arr.push(new_val);
                        } else {
                            println!("Value '{}' already exists in {}.", value, key);
                            return Ok(());
                        }
                    }
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Cannot add to configuration key '{}'. It is not a list.",
                        key
                    ));
                }
            }
            println!("Added '{}' to {}", value, key);
        }
        ConfigCommands::Remove { key, value } => {
            match key.as_str() {
                "locations" | "includes" | "excludes" => {
                    if let Some(entry) = object.get_mut(&key) {
                        if let Some(arr) = entry.as_array_mut() {
                            let val_to_remove = serde_json::Value::String(value.clone());
                            if let Some(pos) = arr.iter().position(|x| *x == val_to_remove) {
                                arr.remove(pos);
                            } else {
                                return Err(anyhow::anyhow!(
                                    "Value '{}' not found in {}",
                                    value,
                                    key
                                ));
                            }
                        }
                    } else {
                        return Err(anyhow::anyhow!("Configuration key '{}' not set.", key));
                    }
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Cannot remove from configuration key '{}'. It is not a list.",
                        key
                    ));
                }
            }
            println!("Removed '{}' from {}", value, key);
        }
        _ => unreachable!(), // Get and List are handled above
    }

    let new_config_str = serde_json::to_string_pretty(&config_json)?;
    std::fs::write(config_path, new_config_str)?;

    Ok(())
}

pub fn run() -> Result<()> {
    ontoenv::api::init_logging();
    let cmd = Cli::parse();
    execute(cmd)
}

pub fn run_from_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    ontoenv::api::init_logging();
    let cmd = Cli::try_parse_from(args).map_err(Error::from)?;
    execute(cmd)
}

fn execute(cmd: Cli) -> Result<()> {
    // The RUST_LOG env var is set by `init_logging` if ONTOENV_LOG is present.
    // CLI flags for verbosity take precedence. If nothing is set, we default to "warn".
    if cmd.debug {
        std::env::set_var("RUST_LOG", "debug");
    } else if cmd.verbose {
        std::env::set_var("RUST_LOG", "info");
    } else if std::env::var("RUST_LOG").is_err() {
        // If no CLI flags and no env var is set, default to "warn".
        std::env::set_var("RUST_LOG", "warn");
    }
    let _ = env_logger::try_init();

    let policy = cmd.policy.unwrap_or_else(|| "default".to_string());

    let mut builder = Config::builder()
        .root(current_dir()?)
        .require_ontology_names(cmd.require_ontology_names)
        .strict(cmd.strict)
        .offline(cmd.offline)
        .resolution_policy(policy)
        .temporary(cmd.temporary)
        .no_search(cmd.no_search);

    // Locations only apply to `init`; other commands ignore positional LOCATIONS
    if let Commands::Init {
        locations: Some(locs),
        ..
    } = &cmd.command
    {
        builder = builder.locations(locs.clone());
    }
    // only set includes if they are provided on the command line, otherwise use builder defaults
    if !cmd.includes.is_empty() {
        builder = builder.includes(&cmd.includes);
    }
    if !cmd.excludes.is_empty() {
        builder = builder.excludes(&cmd.excludes);
    }

    let config: Config = builder.build()?;

    if cmd.verbose || cmd.debug {
        config.print();
    }

    if let Commands::Reset { force } = &cmd.command {
        if let Some(root) = ontoenv::api::find_ontoenv_root() {
            let path = root.join(".ontoenv");
            println!("Removing .ontoenv directory at {}...", path.display());
            if !*force {
                // check delete? [y/N]
                let mut input = String::new();
                println!("Are you sure you want to delete the .ontoenv directory? [y/N] ");
                std::io::stdin()
                    .read_line(&mut input)
                    .expect("Failed to read line");
                let input = input.trim();
                if input != "y" && input != "Y" {
                    println!("Aborting...");
                    return Ok(());
                }
            }
            OntoEnv::reset()?;
            println!(".ontoenv directory removed.");
        } else {
            println!("No .ontoenv directory found. Nothing to do.");
        }
        return Ok(());
    }

    // Discover environment root: ONTOENV_DIR takes precedence, else walk parents
    let env_dir_var = std::env::var("ONTOENV_DIR").ok().map(PathBuf::from);
    let discovered_root = if let Some(dir) = env_dir_var.clone() {
        // If ONTOENV_DIR points to the .ontoenv directory, take its parent as root
        if dir.file_name().map(|n| n == ".ontoenv").unwrap_or(false) {
            dir.parent().map(|p| p.to_path_buf())
        } else {
            Some(dir)
        }
    } else {
        ontoenv::api::find_ontoenv_root()
    };
    let ontoenv_exists = discovered_root
        .as_ref()
        .map(|root| root.join(".ontoenv").join("ontoenv.json").exists())
        .unwrap_or(false);
    info!("OntoEnv exists: {ontoenv_exists}");

    // create the env object to use in the subcommand.
    // - if temporary is true, create a new env object each time
    // - if temporary is false, load the env from the .ontoenv directory if it exists
    // Determine if this command needs write access to the store
    let needs_rw = matches!(cmd.command, Commands::Add { .. } | Commands::Update { .. });

    let env: Option<OntoEnv> = if cmd.temporary {
        // Create a new OntoEnv object in temporary mode
        let e = OntoEnv::init(config.clone(), false)?;
        Some(e)
    } else if cmd.command.to_string() != "Init" && ontoenv_exists {
        // if .ontoenv exists, load it from discovered root
        // Open read-only unless the command requires write access
        Some(OntoEnv::load_from_directory(
            discovered_root.unwrap(),
            !needs_rw,
        )?)
    } else {
        None
    };
    info!("OntoEnv loaded: {}", env.is_some());

    match cmd.command {
        Commands::Init { overwrite, .. } => {
            // if temporary, raise an error
            if cmd.temporary {
                return Err(anyhow::anyhow!(
                    "Cannot initialize in temporary mode. Run `ontoenv init` without --temporary."
                ));
            }

            let root = current_dir()?;
            if root.join(".ontoenv").exists() && !overwrite {
                println!(
                    "An ontology environment already exists in: {}",
                    root.display()
                );
                println!("Use --overwrite to re-initialize or `ontoenv update` to update.");

                let env = OntoEnv::load_from_directory(root, false)?;
                let status = env.status()?;
                println!("\nCurrent status:");
                println!("{status}");
                return Ok(());
            }

            // The call to `init` will create and update the environment.
            // `update` will also save it to the directory.
            let _ = OntoEnv::init(config, overwrite)?;
        }
        Commands::Get {
            ontology,
            location,
            output,
            format,
        } => {
            let env = require_ontoenv(env)?;

            // If a location is provided, resolve by location. Otherwise resolve by name (IRI).
            let graph = if let Some(loc) = location {
                let oloc = if loc.starts_with("http://") || loc.starts_with("https://") {
                    OntologyLocation::Url(loc)
                } else {
                    // Normalize to absolute path
                    ontoenv::ontology::OntologyLocation::from_str(&loc)
                        .unwrap_or_else(|_| OntologyLocation::File(PathBuf::from(loc)))
                };
                // Read directly from the specified location to disambiguate
                oloc.graph()?
            } else {
                let iri = NamedNode::new(ontology).map_err(|e| anyhow::anyhow!(e.to_string()))?;
                let graphid = env
                    .resolve(ResolveTarget::Graph(iri))
                    .ok_or(anyhow::anyhow!("Ontology not found"))?;
                env.get_graph(&graphid)?
            };

            let fmt = match format
                .as_deref()
                .unwrap_or("turtle")
                .to_ascii_lowercase()
                .as_str()
            {
                "turtle" | "ttl" => RdfFormat::Turtle,
                "ntriples" | "nt" => RdfFormat::NTriples,
                "rdfxml" | "xml" => RdfFormat::RdfXml,
                "jsonld" | "json-ld" => RdfFormat::JsonLd {
                    profile: JsonLdProfileSet::default(),
                },
                other => {
                    return Err(anyhow::anyhow!(
                        "Unsupported format '{}'. Use one of: turtle, ntriples, rdfxml, jsonld",
                        other
                    ))
                }
            };

            if let Some(path) = output {
                let mut file = std::fs::File::create(path)?;
                let mut serializer =
                    oxigraph::io::RdfSerializer::from_format(fmt).for_writer(&mut file);
                for t in graph.iter() {
                    serializer.serialize_triple(t)?;
                }
                serializer.finish()?;
            } else {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                let mut serializer =
                    oxigraph::io::RdfSerializer::from_format(fmt).for_writer(&mut handle);
                for t in graph.iter() {
                    serializer.serialize_triple(t)?;
                }
                serializer.finish()?;
            }
        }
        Commands::Version => {
            println!(
                "ontoenv {} @ {}",
                env!("CARGO_PKG_VERSION"),
                env!("GIT_HASH")
            );
        }
        Commands::Status { json } => {
            let env = require_ontoenv(env)?;
            if json {
                // Recompute status details similar to env.status()
                let ontoenv_dir = current_dir()?.join(".ontoenv");
                let last_updated = if ontoenv_dir.exists() {
                    Some(std::fs::metadata(&ontoenv_dir)?.modified()?)
                        as Option<std::time::SystemTime>
                } else {
                    None
                };
                let size: u64 = if ontoenv_dir.exists() {
                    walkdir::WalkDir::new(&ontoenv_dir)
                        .into_iter()
                        .filter_map(Result::ok)
                        .filter(|e| e.file_type().is_file())
                        .filter_map(|e| e.metadata().ok())
                        .map(|m| m.len())
                        .sum()
                } else {
                    0
                };
                let missing: Vec<String> = env
                    .missing_imports()
                    .into_iter()
                    .map(|n| n.to_uri_string())
                    .collect();
                let last_str =
                    last_updated.map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
                let obj = serde_json::json!({
                    "exists": true,
                    "num_ontologies": env.ontologies().len(),
                    "last_updated": last_str,
                    "store_size_bytes": size,
                    "missing_imports": missing,
                });
                println!("{}", serde_json::to_string_pretty(&obj)?);
            } else {
                let status = env.status()?;
                println!("{status}");
            }
        }
        Commands::Update { quiet, all, json } => {
            let mut env = require_ontoenv(env)?;
            let updated = env.update_all(all)?;
            if json {
                let arr: Vec<String> = updated.iter().map(|id| id.to_uri_string()).collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
            } else if !quiet {
                for id in updated {
                    if let Some(ont) = env.ontologies().get(&id) {
                        let name = ont.name().to_string();
                        let loc = ont
                            .location()
                            .map(|l| l.to_string())
                            .unwrap_or_else(|| "N/A".to_string());
                        println!("{} @ {}", name, loc);
                    }
                }
            }
            env.save_to_directory()?;
        }
        Commands::Closure {
            ontology,
            no_rewrite_sh_prefixes,
            keep_owl_imports,
            destination,
            recursion_depth,
        } => {
            // make ontology an IRI
            let iri = NamedNode::new(ontology).map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let env = require_ontoenv(env)?;
            let graphid = env
                .resolve(ResolveTarget::Graph(iri.clone()))
                .ok_or(anyhow::anyhow!(format!("Ontology {} not found", iri)))?;
            let closure = env.get_closure(&graphid, recursion_depth)?;
            // Defaults: rewrite prefixes = ON, remove owl:imports = ON; flags disable these.
            let rewrite = !no_rewrite_sh_prefixes;
            let remove = !keep_owl_imports;
            let union = env.get_union_graph(&closure, Some(rewrite), Some(remove))?;
            if let Some(failed_imports) = union.failed_imports {
                for imp in failed_imports {
                    eprintln!("{imp}");
                }
            }
            // write the graph to a file
            let destination = destination.unwrap_or_else(|| "output.ttl".to_string());
            write_dataset_to_file(&union.dataset, &destination)?;
        }
        Commands::Add {
            location,
            no_imports,
        } => {
            let location = if location.starts_with("http") {
                OntologyLocation::Url(location)
            } else {
                OntologyLocation::File(PathBuf::from(location))
            };
            let mut env = require_ontoenv(env)?;
            if no_imports {
                let _ =
                    env.add_no_imports(location, Overwrite::Allow, RefreshStrategy::UseCache)?;
            } else {
                let _ = env.add(location, Overwrite::Allow, RefreshStrategy::UseCache)?;
            }
        }
        Commands::List { list_cmd, json } => {
            let env = require_ontoenv(env)?;
            match list_cmd {
                ListCommands::Locations => {
                    let mut locations = env.find_files()?;
                    locations.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                    if json {
                        println!("{}", serde_json::to_string_pretty(&locations)?);
                    } else {
                        for loc in locations {
                            println!("{}", loc);
                        }
                    }
                }
                ListCommands::Ontologies => {
                    // print list of ontology URLs from env.ontologies.values() sorted alphabetically
                    let mut ontologies: Vec<&GraphIdentifier> = env.ontologies().keys().collect();
                    ontologies.sort_by(|a, b| a.name().cmp(&b.name()));
                    ontologies.dedup_by(|a, b| a.name() == b.name());
                    if json {
                        let out: Vec<String> =
                            ontologies.into_iter().map(|o| o.to_uri_string()).collect();
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        for ont in ontologies {
                            println!("{}", ont.to_uri_string());
                        }
                    }
                }
                ListCommands::Missing => {
                    let mut missing_imports = env.missing_imports();
                    missing_imports.sort();
                    if json {
                        let out: Vec<String> = missing_imports
                            .into_iter()
                            .map(|n| n.to_uri_string())
                            .collect();
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        for import in missing_imports {
                            println!("{}", import.to_uri_string());
                        }
                    }
                }
            }
        }
        Commands::Dump { contains } => {
            let env = require_ontoenv(env)?;
            env.dump(contains.as_deref());
        }
        Commands::DepGraph { roots, output } => {
            let env = require_ontoenv(env)?;
            let dot = if let Some(roots) = roots {
                let roots: Vec<GraphIdentifier> = roots
                    .iter()
                    .map(|iri| {
                        env.resolve(ResolveTarget::Graph(NamedNode::new(iri).unwrap()))
                            .unwrap()
                            .clone()
                    })
                    .collect();
                env.rooted_dep_graph_to_dot(roots)?
            } else {
                env.dep_graph_to_dot()?
            };
            // call graphviz to generate PDF
            let dot_path = current_dir()?.join("dep_graph.dot");
            std::fs::write(&dot_path, dot)?;
            let output_path = output.unwrap_or_else(|| "dep_graph.pdf".to_string());
            let output = std::process::Command::new("dot")
                .args(["-Tpdf", dot_path.to_str().unwrap(), "-o", &output_path])
                .output()?;
            if !output.status.success() {
                return Err(anyhow::anyhow!(
                    "Failed to generate PDF: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        Commands::Why { ontologies, json } => {
            let env = require_ontoenv(env)?;
            if json {
                let mut all: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
                for ont in ontologies {
                    let iri = NamedNode::new(ont).map_err(|e| anyhow::anyhow!(e.to_string()))?;
                    let (paths, missing) = match env.explain_import(&iri)? {
                        ontoenv::api::ImportPaths::Present(paths) => (paths, false),
                        ontoenv::api::ImportPaths::Missing { importers } => (importers, true),
                    };
                    let formatted = format_import_paths(&iri, paths, missing);
                    all.insert(iri.to_uri_string(), formatted);
                }
                println!("{}", serde_json::to_string_pretty(&all)?);
            } else {
                for ont in ontologies {
                    let iri = NamedNode::new(ont).map_err(|e| anyhow::anyhow!(e.to_string()))?;
                    match env.explain_import(&iri)? {
                        ontoenv::api::ImportPaths::Present(paths) => {
                            print_import_paths(&iri, paths, false);
                        }
                        ontoenv::api::ImportPaths::Missing { importers } => {
                            print_import_paths(&iri, importers, true);
                        }
                    }
                }
            }
        }
        Commands::Doctor { json } => {
            let env = require_ontoenv(env)?;
            let problems = env.doctor()?;
            if json {
                let out: Vec<serde_json::Value> = problems
                    .into_iter()
                    .map(|p| serde_json::json!({
                        "message": p.message,
                        "locations": p.locations.into_iter().map(|loc| loc.to_string()).collect::<Vec<_>>()
                    }))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else if problems.is_empty() {
                println!("No issues found.");
            } else {
                println!("Found {} issues:", problems.len());
                for problem in problems {
                    println!("- {}", problem.message);
                    for location in problem.locations {
                        println!("  - {location}");
                    }
                }
            }
        }
        Commands::Config(config_cmd) => {
            handle_config_command(config_cmd, cmd.temporary)?;
        }
        Commands::Reset { .. } => {
            // This command is handled before the environment is loaded.
        }
    }

    Ok(())
}

fn require_ontoenv(env: Option<OntoEnv>) -> Result<OntoEnv> {
    env.ok_or_else(|| {
        anyhow::anyhow!("OntoEnv not found. Run `ontoenv init` to create a new OntoEnv or use -t/--temporary to use a temporary environment.")
    })
}

fn format_import_paths(
    target: &NamedNode,
    paths: Vec<Vec<GraphIdentifier>>,
    missing: bool,
) -> Vec<Vec<String>> {
    let mut unique: BTreeSet<Vec<String>> = BTreeSet::new();
    if paths.is_empty() {
        if missing {
            unique.insert(vec![format!("{} (missing)", target.to_uri_string())]);
        }
        return unique.into_iter().collect();
    }
    for path in paths {
        let mut entries: Vec<String> = path.into_iter().map(|id| id.to_uri_string()).collect();
        if missing {
            entries.push(format!("{} (missing)", target.to_uri_string()));
        }
        unique.insert(entries);
    }
    unique.into_iter().collect()
}

fn print_import_paths(target: &NamedNode, paths: Vec<Vec<GraphIdentifier>>, missing: bool) {
    if paths.is_empty() {
        if missing {
            println!(
                "Ontology {} is missing but no importers reference it.",
                target.to_uri_string()
            );
        } else {
            println!("No importers found for {}", target.to_uri_string());
        }
        return;
    }

    println!(
        "Why {}{}:",
        target.to_uri_string(),
        if missing { " (missing)" } else { "" }
    );

    let mut lines: BTreeSet<String> = BTreeSet::new();
    for path in paths {
        let mut segments: Vec<String> = path.into_iter().map(|id| id.to_uri_string()).collect();
        if missing {
            segments.push(format!("{} (missing)", target.to_uri_string()));
        }
        lines.insert(segments.join(" -> "));
    }

    for line in lines {
        println!("{}", line);
    }
}
