#![feature(let_chains)]
use anyhow::Result;
use clap::{Parser, Subcommand};
use ontoenv::api::{OntoEnv, ResolveTarget};
use ontoenv::config::{Config, EnvironmentConfig};
use ontoenv::ontology::{GraphIdentifier, OntologyLocation};
use ontoenv::util::write_dataset_to_file;
use oxigraph::model::NamedNode;
use serde_json;
use std::env::current_dir;
use std::fs::File;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "ontoenv")]
#[command(about = "Ontology environment manager")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Verbose mode - sets the RUST_LOG level to info, defaults to warning level
    #[clap(long, short, action, default_value = "false")]
    verbose: bool,
    /// Debug mode - sets the RUST_LOG level to debug, defaults to warning level
    #[clap(long, action, default_value = "false")]
    debug: bool,
    /// Directories to search for ontologies. If not provided, the current directory is used.
    #[clap(long, short, num_args = 1..)]
    search_directories: Option<Vec<PathBuf>>,
    /// Resolution policy for determining which ontology to use when there are multiple with the same name
    #[clap(long, short, default_value = "default")]
    policy: Option<String>,
    /// Temporary (non-persistent) mode - will not save the environment to disk
    #[clap(long, short, action)]
    temporary: bool,
    /// Require ontology names to be unique; will raise an error if multiple ontologies have the same name
    #[clap(long, action)]
    require_ontology_names: bool,
    /// Strict mode - will raise an error if an ontology is not found
    #[clap(long, action, default_value = "false")]
    strict: bool,
    /// Offline mode - will not attempt to fetch ontologies from the web
    #[clap(long, short, action, default_value = "false")]
    offline: bool,
    /// Glob patterns for which files to include, defaults to ['*.ttl','*.xml','*.n3']
    #[clap(long, short, num_args = 1..)]
    includes: Vec<String>,
    /// Glob patterns for which files to exclude, defaults to []
    #[clap(long, short, num_args = 1..)]
    excludes: Vec<String>,
    /// Do not search for ontologies in the search directories
    #[clap(long = "no-search", short = 'n', action)]
    no_search: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create a new ontology environment
    Init {
        /// Overwrite the environment if it already exists
        #[clap(long, short, default_value = "false")]
        overwrite: bool,
        /// A JSON file containing a list of ontologies to add to the environment
        #[clap(long = "list", short = 'l')]
        ontology_list_file: Option<String>,
    },
    /// Prints the version of the ontoenv binary
    Version,
    /// Prints the status of the ontology environment
    Status,
    /// Update the ontology environment
    Refresh,
    /// Compute the owl:imports closure of an ontology and write it to a file
    GetClosure {
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
    /// List the ontologies in the environment sorted by name
    ListOntologies,
    /// List the locations of the ontologies in the environment sorted by location
    ListLocations,
    // TODO: dump all ontologies; nest by ontology name (sorted), w/n each ontology name list all
    // the places where that graph can be found. List basic stats: the metadata field in the
    // Ontology struct and # of triples in the graph; last updated; etc
    /// Print out the current state of the ontology environment
    Dump {
        /// Filter the output to only include ontologies that contain the given string in their
        /// name
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
    Reset,
}

fn main() -> Result<()> {
    let cmd = Cli::parse();

    let log_level = if cmd.verbose { "info" } else { "warn" };
    let log_level = if cmd.debug { "debug" } else { log_level };
    std::env::set_var("RUST_LOG", log_level);
    env_logger::init();

    let policy = cmd.policy.unwrap_or_else(|| "default".to_string());

    let config: Config = Config::new(
        current_dir()?,
        cmd.search_directories,
        &cmd.includes,
        &cmd.excludes,
        cmd.require_ontology_names,
        cmd.strict,
        cmd.offline,
        policy,
        false,
        cmd.temporary,
    )?;

    // create the env object to use in the subcommand.
    // - if temporary is true, create a new env object each time
    // - if temporary is false, load the env from the .ontoenv directory if it exists
    let mut env = if cmd.temporary {
        let mut env = OntoEnv::init(config, false)?;
        env.update()?;
        env
    } else {
        // if the command is NOT init and the .ontoenv directory doesn't exist, raise an error
        let path = current_dir()?;
        if let Commands::Init { .. } = cmd.command && !path.exists() {
            return Err(anyhow::anyhow!(
                "OntoEnv not found. Run `ontoenv init` to create a new OntoEnv."
            ));
        }
        OntoEnv::load_from_directory(path)?
    };

    match cmd.command {
        Commands::Init {
            overwrite,
            ontology_list_file,
        } => {
            // if temporary, raise an error
            if cmd.temporary {
                return Err(anyhow::anyhow!(
                    "Cannot initialize in temporary mode. Run `ontoenv init` to create a new OntoEnv."
                ));
            }
            // if an ontology config file is provided, load it and add the ontologies
            if let Some(file) = ontology_list_file {
                let file = File::open(file)?;
                let config: EnvironmentConfig = serde_json::from_reader(file)?;
                for ont in config.ontologies {
                    env.add(ont.location, true)?;
                }
            }

            env.update()?;
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
            // load env from .ontoenv/ontoenv.json
            let status = env.status()?;
            // pretty print the status
            println!("{}", status);
        }
        Commands::Refresh => {
            // if temporary, raise an error
            if cmd.temporary {
                return Err(anyhow::anyhow!(
                    "Cannot refresh in temporary mode. Run `ontoenv init` to create a new OntoEnv."
                ));
            }
            env.update()?;
            env.save_to_directory()?;
        }
        Commands::GetClosure {
            ontology,
            rewrite_sh_prefixes,
            remove_owl_imports,
            destination,
        } => {
            // make ontology an IRI
            let iri = NamedNode::new(ontology).map_err(|e| anyhow::anyhow!(e.to_string()))?;

            let graphid = env
                .resolve(ResolveTarget::Graph(iri.clone()))
                .ok_or(anyhow::anyhow!(format!("Ontology {} not found", iri)))?;
            let closure = env.get_dependency_closure(&graphid)?;
            let (graph, _successful, failed_imports) =
                env.get_union_graph(&closure, rewrite_sh_prefixes, remove_owl_imports)?;
            if let Some(failed_imports) = failed_imports {
                for imp in failed_imports {
                    eprintln!("{}", imp);
                }
            }
            // write the graph to a file
            if let Some(destination) = destination {
                write_dataset_to_file(&graph, &destination)?;
            } else {
                write_dataset_to_file(&graph, "output.ttl")?;
            }
        }
        Commands::Add { url, file } => {
            let location: OntologyLocation = match (url, file) {
                (Some(url), None) => OntologyLocation::Url(url),
                (None, Some(file)) => OntologyLocation::File(PathBuf::from(file)),
                _ => return Err(anyhow::anyhow!("Must specify either --url or --file")),
            };

            env.add(location, true)?;
            env.save_to_directory()?;
        }
        Commands::ListOntologies => {
            // print list of ontology URLs from env.onologies.values() sorted alphabetically
            let mut ontologies: Vec<&GraphIdentifier> = env.ontologies().keys().collect();
            ontologies.sort_by(|a, b| a.name().cmp(&b.name()));
            ontologies.dedup_by(|a, b| a.name() == b.name());
            for ont in ontologies {
                println!("{}", ont.name().as_str());
            }
        }
        Commands::ListLocations => {
            let mut ontologies: Vec<&GraphIdentifier> = env.ontologies().keys().collect();
            ontologies.sort_by(|a, b| a.location().as_str().cmp(b.location().as_str()));
            for ont in ontologies {
                println!("{}", ont.location().as_str());
            }
        }
        Commands::Dump { contains } => {
            env.dump(contains.as_deref());
        }
        Commands::DepGraph { roots, output } => {
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
            for ont in ontologies {
                let iri = NamedNode::new(ont).map_err(|e| anyhow::anyhow!(e.to_string()))?;
                let dependents = env.get_dependents(&iri)?;
                println!("Dependents of {}: ", iri);
                for dep in dependents {
                    println!("{}", dep);
                }
            }
        }
        Commands::Doctor => {
            env.doctor();
        }
        Commands::Reset => {
            // remove .ontoenv directory
            let path = current_dir()?.join(".ontoenv");
            if path.exists() {
                std::fs::remove_dir_all(path)?;
            }
        }
    }

    Ok(())
}
