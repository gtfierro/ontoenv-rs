use anyhow::Result;
use clap::{Parser, Subcommand};
use log::info;
use ontoenv::api::{OntoEnv, ResolveTarget};
use ontoenv::config::Config;
use ontoenv::ontology::{GraphIdentifier, OntologyLocation};
use ontoenv::util::write_dataset_to_file;
use ontoenv::ToUriString;
use oxigraph::model::NamedNode;
use serde_json;
use std::env::current_dir;
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
    /// Directories to search for ontologies. If not provided, the current directory is used.
    #[clap(global = true)]
    locations: Option<Vec<PathBuf>>,
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
    },
    /// Prints the version of the ontoenv binary
    Version,
    /// Prints the status of the ontology environment
    Status,
    /// Update the ontology environment
    Update,
    /// Compute the owl:imports closure of an ontology and write it to a file
    Closure {
        /// The name (URI) of the ontology to compute the closure for
        ontology: String,
        /// Rewrite the sh:prefixes declarations to point to the chosen ontology, defaults to true
        #[clap(long, short, action, default_value = "true")]
        rewrite_sh_prefixes: Option<bool>,
        /// Remove owl:imports statements from the closure, defaults to true
        #[clap(long, short, action, default_value = "true")]
        remove_owl_imports: Option<bool>,
        /// The file to write the closure to, defaults to 'output.ttl'
        destination: Option<String>,
    },
    /// Add an ontology to the environment
    Add {
        /// The URL of the ontology to add
        #[clap(long, short)]
        url: Option<String>,
        /// The path to the file to add
        #[clap(long, short)]
        file: Option<String>,
    },
    /// List various properties of the environment
    #[command(subcommand)]
    List(ListCommands),
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
    /// Lists all ontologies which depend on the given ontology
    Dependents {
        /// The name (URI) of the ontology to find dependents for
        ontologies: Vec<String>,
    },
    /// Run the doctor to check the environment for issues
    Doctor,
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
            Commands::Status => "Status".to_string(),
            Commands::Update => "Update".to_string(),
            Commands::Closure { .. } => "Closure".to_string(),
            Commands::Add { .. } => "Add".to_string(),
            Commands::List(..) => "List".to_string(),
            Commands::Dump { .. } => "Dump".to_string(),
            Commands::DepGraph { .. } => "DepGraph".to_string(),
            Commands::Dependents { .. } => "Dependents".to_string(),
            Commands::Doctor => "Doctor".to_string(),
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

fn main() -> Result<()> {
    let cmd = Cli::parse();

    let log_level = if cmd.verbose { "info" } else { "warn" };
    let log_level = if cmd.debug { "debug" } else { log_level };
    std::env::set_var("RUST_LOG", log_level);
    env_logger::init();

    let policy = cmd.policy.unwrap_or_else(|| "default".to_string());

    let mut builder = Config::builder()
        .root(current_dir()?)
        .require_ontology_names(cmd.require_ontology_names)
        .strict(cmd.strict)
        .offline(cmd.offline)
        .resolution_policy(policy)
        .temporary(cmd.temporary)
        .no_search(cmd.no_search);

    if let Some(locations) = cmd.locations {
        builder = builder.locations(locations);
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

    let ontoenv_exists = ontoenv::api::find_ontoenv_root()
        .map(|root| root.join(".ontoenv").join("ontoenv.json").exists())
        .unwrap_or(false);
    info!("OntoEnv exists: {ontoenv_exists}");

    // create the env object to use in the subcommand.
    // - if temporary is true, create a new env object each time
    // - if temporary is false, load the env from the .ontoenv directory if it exists
    let env: Option<OntoEnv> = if cmd.temporary {
        // Create a new OntoEnv object in temporary mode
        let e = OntoEnv::init(config.clone(), false)?;
        Some(e)
    } else if cmd.command.to_string() != "Init" && ontoenv_exists {
        // if .ontoenv exists, load it
        Some(OntoEnv::load_from_directory(current_dir()?, false)?) // no read-only
    } else {
        None
    };
    info!("OntoEnv loaded: {}", env.is_some());

    match cmd.command {
        Commands::Init { overwrite } => {
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

            let env = OntoEnv::init(config, overwrite)?;
            env.save_to_directory()?;
        }
        Commands::Version => {
            println!(
                "ontoenv {} @ {}",
                env!("CARGO_PKG_VERSION"),
                env!("GIT_HASH")
            );
        }
        Commands::Status => {
            let env = require_ontoenv(env)?;
            // load env from .ontoenv/ontoenv.json
            let status = env.status()?;
            // pretty print the status
            println!("{status}");
        }
        Commands::Update => {
            let mut env = require_ontoenv(env)?;
            env.update()?;
            env.save_to_directory()?;
        }
        Commands::Closure {
            ontology,
            rewrite_sh_prefixes,
            remove_owl_imports,
            destination,
        } => {
            // make ontology an IRI
            let iri = NamedNode::new(ontology).map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let env = require_ontoenv(env)?;
            let graphid = env
                .resolve(ResolveTarget::Graph(iri.clone()))
                .ok_or(anyhow::anyhow!(format!("Ontology {} not found", iri)))?;
            let closure = env.get_closure(&graphid)?;
            let union = env.get_union_graph(&closure, rewrite_sh_prefixes, remove_owl_imports)?;
            if let Some(failed_imports) = union.failed_imports {
                for imp in failed_imports {
                    eprintln!("{imp}");
                }
            }
            // write the graph to a file
            let destination = destination.unwrap_or_else(|| "output.ttl".to_string());
            write_dataset_to_file(&union.dataset, &destination)?;
        }
        Commands::Add { url, file } => {
            let location: OntologyLocation = match (url, file) {
                (Some(url), None) => OntologyLocation::Url(url),
                (None, Some(file)) => OntologyLocation::File(PathBuf::from(file)),
                _ => return Err(anyhow::anyhow!("Must specify either --url or --file")),
            };
            let mut env = require_ontoenv(env)?;
            let _ = env.add(location, true)?;
            env.save_to_directory()?;
        }
        Commands::List(list_cmd) => {
            let env = require_ontoenv(env)?;
            match list_cmd {
                ListCommands::Locations => {
                    let mut locations = env.find_files()?;
                    locations.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                    for loc in locations {
                        println!("{}", loc);
                    }
                }
                ListCommands::Ontologies => {
                    // print list of ontology URLs from env.ontologies.values() sorted alphabetically
                    let mut ontologies: Vec<&GraphIdentifier> = env.ontologies().keys().collect();
                    ontologies.sort_by(|a, b| a.name().cmp(&b.name()));
                    ontologies.dedup_by(|a, b| a.name() == b.name());
                    for ont in ontologies {
                        println!("{}", ont.to_uri_string());
                    }
                }
                ListCommands::Missing => {
                    let mut missing_imports = env.missing_imports();
                    missing_imports.sort();
                    for import in missing_imports {
                        println!("{}", import.to_uri_string());
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
        Commands::Dependents { ontologies } => {
            let env = require_ontoenv(env)?;
            for ont in ontologies {
                let iri = NamedNode::new(ont).map_err(|e| anyhow::anyhow!(e.to_string()))?;
                let dependents = env.get_dependents(&iri)?;
                println!("Dependents of {}: ", iri.to_uri_string());
                for dep in dependents {
                    println!("{}", dep.to_uri_string());
                }
            }
        }
        Commands::Doctor => {
            let env = require_ontoenv(env)?;
            let problems = env.doctor()?;
            if problems.is_empty() {
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
