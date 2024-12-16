use hashbrown::HashSet;
use itertools::Itertools;
use rand::{thread_rng, Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rust::{
    check_probabilisitic_equivalence,
    circuit::{BaseGate, Circuit},
    prepare_circuit, run_local_mixing, toposort_with_cached_graph_neighbours,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env::{self, args},
    error::Error,
    io::Read,
    path::Path,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Strategy {
    Strategy1,
    Strategy2,
}

#[derive(Serialize, Deserialize)]
struct ObfuscationConfig {
    // Number of wires
    n: usize,
    // Total steps in strategy 1
    total_steps: usize,
    // Number of inflationary steps in strategy 2
    inflationary_stage_steps: usize,
    // Number of kneading steps strategy 2
    kneading_stage_steps: usize,
    // Maximum number of iterations for each convex searching
    max_convex_iterations: usize,
    // Maximum number of iterations for each replacement circuit searching
    max_replacement_iterations: usize,
    // Strategy used
    starategy: Strategy,
    // Checkpoint steps. Checkpoints obfuscated circuit after `checkpoint` number of iterations
    checkpoint_steps: usize,
    // No. of iterations for probabilitic equivalance check.
    probabilitic_eq_check_iterations: usize,
}

impl ObfuscationConfig {
    // This function creates a new `ObfuscationConfig` with parameters specific to Strategy 1.
    // It initializes the configuration with the given number of wires, total steps, and iteration limits.
    // The inflationary and kneading stage steps are set to zero as they are not used in Strategy 1.
    fn new_with_strategy1(
        n: usize,
        total_steps: usize,
        max_convex_iterations: usize,
        max_replacement_iterations: usize,
        checkpoint_steps: usize,
        probabilitic_eq_check_iterations: usize,
    ) -> Self {
        Self {
            n: n,
            total_steps: total_steps,
            inflationary_stage_steps: 0,
            kneading_stage_steps: 0,
            max_convex_iterations,
            max_replacement_iterations,
            starategy: Strategy::Strategy1,
            checkpoint_steps,
            probabilitic_eq_check_iterations,
        }
    }

    // This function creates a new `ObfuscationConfig` with parameters specific to Strategy 2.
    // It initializes the configuration with the given number of wires, inflationary and kneading stage steps, and iteration limits.
    // The total steps are set to zero as they are not used in Strategy 2.
    fn new_with_strategy2(
        n: usize,
        inflationary_stage_steps: usize,
        kneading_stage_steps: usize,
        max_convex_iterations: usize,
        max_replacement_iterations: usize,
        checkpoint_steps: usize,
        probabilitic_eq_check_iterations: usize,
    ) -> Self {
        Self {
            n,
            inflationary_stage_steps,
            kneading_stage_steps,
            max_convex_iterations,
            max_replacement_iterations,
            starategy: Strategy::Strategy2,
            total_steps: 0,
            checkpoint_steps,
            probabilitic_eq_check_iterations,
        }
    }

    // This function returns a default `ObfuscationConfig` for Strategy 1 with pre-defined parameters.
    fn default_strategy1() -> Self {
        ObfuscationConfig::new_with_strategy1(64, 100_000, 100_000, 10_000_000, 1000, 1000)
    }

    // This function returns a default `ObfuscationConfig` for Strategy 2 with pre-defined parameters.
    fn default_strategy2() -> Self {
        ObfuscationConfig::new_with_strategy2(64, 100_000, 100_000, 10000, 1000000, 1000, 1000)
    }
}

#[derive(Serialize, Deserialize)]
struct ObfuscationJob {
    config: ObfuscationConfig,
    // [Strategy 1] Curr no. of total steps
    curr_total_steps: usize,
    // [Strategy 2] Curr no. of steps in inflationary stage
    curr_inflationary_stage_steps: usize,
    // [Strategy 2] Curr no. of steps in kneading stage
    curr_kneading_stage_steps: usize,
    curr_circuit: Circuit<BaseGate<2, u8>>,
    original_circuit: Circuit<BaseGate<2, u8>>,
}

impl ObfuscationJob {
    // This function loads an `ObfuscationJob` from a file at the given path.
    // It deserializes the job from a binary format and logs the job's status.
    fn load(path: impl AsRef<Path>) -> Self {
        let job: ObfuscationJob = bincode::deserialize(&std::fs::read(path).unwrap()).unwrap();

        #[allow(dead_code)]
        #[derive(Debug)]
        struct Status {
            n: usize,
            total_steps: usize,
            inflationary_stage_steps: usize,
            kneading_stage_steps: usize,
            max_convex_iterations: usize,
            max_replacement_iterations: usize,
            starategy: Strategy,
            checkpoint_steps: usize,
            curr_total_steps: usize,
            curr_inflationary_stage_steps: usize,
            curr_kneading_stage_steps: usize,
            curr_circuit_digest: String,
            original_circuit_digest: String,
        }

        log::info!(
            "loaded job: {:#?}",
            Status {
                n: job.config.n,
                total_steps: job.config.total_steps,
                inflationary_stage_steps: job.config.inflationary_stage_steps,
                kneading_stage_steps: job.config.kneading_stage_steps,
                max_convex_iterations: job.config.max_convex_iterations,
                max_replacement_iterations: job.config.max_replacement_iterations,
                starategy: job.config.starategy,
                checkpoint_steps: job.config.checkpoint_steps,
                curr_total_steps: job.curr_total_steps,
                curr_inflationary_stage_steps: job.curr_inflationary_stage_steps,
                curr_kneading_stage_steps: job.curr_kneading_stage_steps,
                curr_circuit_digest: hex::encode(Sha256::digest(
                    bincode::serialize(&job.curr_circuit).unwrap()
                )),
                original_circuit_digest: hex::encode(Sha256::digest(
                    bincode::serialize(&job.original_circuit).unwrap()
                )),
            }
        );

        job
    }

    // This function stores the current state of an `ObfuscationJob` to a file at the given path.
    // It serializes the job to a binary format and logs the current state of the job.
    fn store(&self, path: impl AsRef<Path>) {
        std::fs::write(&path, bincode::serialize(self).unwrap()).unwrap();

        log::info!(
            "stored job, curr_inflationary_stage_steps: {}, curr_kneading_stage_steps: {}, curr_circuit digest: 0x{}, original_circuit digest: 0x{}",
            self.curr_inflationary_stage_steps,
            self.curr_kneading_stage_steps,
            hex::encode(Sha256::digest(bincode::serialize(&self.curr_circuit).unwrap())),
            hex::encode(Sha256::digest(bincode::serialize(&self.original_circuit).unwrap())),
        );
    }
}

// This function executes Strategy 1 for an obfuscation job.
// It performs a series of local mixing steps on the circuit, updating the job's state and storing checkpoints as needed.
fn run_strategy1(job: &mut ObfuscationJob, job_path: String, debug: bool) {
    let original_circuit = job.original_circuit.clone();
    let mut rng = ChaCha8Rng::from_entropy();

    let (
        mut direct_connections,
        mut direct_incoming_connections,
        mut skeleton_graph,
        mut gate_id_to_node_index_map,
        mut gate_map,
        mut graph_neighbours,
        mut active_edges_with_gateids,
        mut latest_id,
    ) = prepare_circuit(&original_circuit);

    // For total no. of steps do the following:
    //  -> Sample a random no. betwee [2, 4]. Set that as ell_out
    //  -> Run local mixing step with ell_out and ell_in = 4

    let mut removed_nodes = HashSet::new();

    while job.curr_total_steps < job.config.total_steps {
        let ell_out = rng.gen_range(2..=4);
        let to_checkpoint = job.curr_total_steps % job.config.checkpoint_steps == 0;

        let success = run_local_mixing(
            &format!(
                "[Strategy 1] [ell^out = {}] Mixing stage step {}",
                ell_out, job.curr_total_steps
            ),
            Some(&original_circuit),
            &mut skeleton_graph,
            &mut direct_connections,
            &mut direct_incoming_connections,
            &mut gate_map,
            &mut gate_id_to_node_index_map,
            &mut graph_neighbours,
            &mut removed_nodes,
            &mut active_edges_with_gateids,
            &mut latest_id,
            job.config.n as u8,
            &mut rng,
            ell_out,
            4,
            job.config.max_convex_iterations,
            job.config.max_replacement_iterations,
            to_checkpoint,
            job.config.probabilitic_eq_check_iterations,
            |mixed_circuit| {
                job.curr_circuit = mixed_circuit;
                job.store(&job_path);
            },
            debug,
        );
        if success {
            job.curr_total_steps += 1;
        }
    }

    {
        let top_sorted_nodes = toposort_with_cached_graph_neighbours(
            &skeleton_graph,
            &graph_neighbours,
            &removed_nodes,
        );
        job.curr_total_steps = job.config.total_steps;
        job.curr_circuit = Circuit::from_top_sorted_nodes(
            &top_sorted_nodes,
            &skeleton_graph,
            &gate_map,
            job.config.n as _,
        );

        let (is_correct, diff_indices) = check_probabilisitic_equivalence(
            &job.curr_circuit,
            &original_circuit,
            job.config.probabilitic_eq_check_iterations,
            &mut rng,
        );
        if !is_correct {
            log::error!(
                "[Error] [Strategy 1] Failed at end of Mixing stage. Different at indices {:?}",
                diff_indices
            );
            assert!(false);
        }

        job.store(&job_path);
    }
}

// This function executes Strategy 2 for an obfuscation job.
// It consists of two stages: inflationary and kneading, each performing local mixing steps on the circuit.
fn run_strategy2(job: &mut ObfuscationJob, job_path: String, debug: bool) {
    let original_circuit = job.original_circuit.clone();
    let mut rng = ChaCha8Rng::from_entropy();

    let (
        mut direct_connections,
        mut direct_incoming_connections,
        mut skeleton_graph,
        mut gate_id_to_node_index_map,
        mut gate_map,
        mut graph_neighbours,
        mut active_edges_with_gateids,
        mut latest_id,
    ) = prepare_circuit(&original_circuit);

    let mut removed_nodes = HashSet::new();

    // Inflationary stage
    {
        while job.curr_inflationary_stage_steps < job.config.inflationary_stage_steps {
            let to_checkpoint =
                job.curr_inflationary_stage_steps % job.config.checkpoint_steps == 0;

            // Inflationary stage
            let success = run_local_mixing(
                &format!(
                    "[Strategy 2] Inflationary stage step {}",
                    job.curr_inflationary_stage_steps
                ),
                Some(&original_circuit),
                &mut skeleton_graph,
                &mut direct_connections,
                &mut direct_incoming_connections,
                &mut gate_map,
                &mut gate_id_to_node_index_map,
                &mut graph_neighbours,
                &mut removed_nodes,
                &mut active_edges_with_gateids,
                &mut latest_id,
                job.config.n as u8,
                &mut rng,
                2,
                4,
                job.config.max_convex_iterations,
                job.config.max_replacement_iterations,
                to_checkpoint,
                job.config.probabilitic_eq_check_iterations,
                |mixed_circuit| {
                    job.curr_circuit = mixed_circuit;
                    job.store(&job_path);
                },
                debug,
            );
            if success {
                job.curr_inflationary_stage_steps += 1;
            }
        }

        {
            let top_sorted_nodes = toposort_with_cached_graph_neighbours(
                &skeleton_graph,
                &graph_neighbours,
                &removed_nodes,
            );
            job.curr_inflationary_stage_steps = job.config.inflationary_stage_steps;
            job.curr_circuit = Circuit::from_top_sorted_nodes(
                &top_sorted_nodes,
                &skeleton_graph,
                &gate_map,
                job.config.n as _,
            );

            let (is_correct, diff_indices) = check_probabilisitic_equivalence(
                &job.curr_circuit,
                &original_circuit,
                job.config.probabilitic_eq_check_iterations,
                &mut rng,
            );
            if !is_correct {
                log::error!(
                    "[Error] [Strategy 2] Failed at end of Inflationary stage. Different at indices {:?}",
                    diff_indices
                );
                assert!(false);
            }

            job.store(&job_path);
        }
    }

    // Kneading stage
    {
        while job.curr_kneading_stage_steps < job.config.kneading_stage_steps {
            let to_checkpoint = job.curr_kneading_stage_steps % job.config.checkpoint_steps == 0;

            let success = run_local_mixing(
                &format!(
                    "[Strategy 2] Kneading stage step {}",
                    job.curr_kneading_stage_steps
                ),
                Some(&original_circuit),
                &mut skeleton_graph,
                &mut direct_connections,
                &mut direct_incoming_connections,
                &mut gate_map,
                &mut gate_id_to_node_index_map,
                &mut graph_neighbours,
                &mut removed_nodes,
                &mut active_edges_with_gateids,
                &mut latest_id,
                job.config.n as u8,
                &mut rng,
                4,
                4,
                job.config.max_convex_iterations,
                job.config.max_replacement_iterations,
                to_checkpoint,
                job.config.probabilitic_eq_check_iterations,
                |mixed_circuit| {
                    job.curr_circuit = mixed_circuit;
                    job.store(&job_path);
                },
                debug,
            );

            if success {
                job.curr_kneading_stage_steps += 1
            }
        }

        {
            let top_sorted_nodes = toposort_with_cached_graph_neighbours(
                &skeleton_graph,
                &graph_neighbours,
                &removed_nodes,
            );
            job.curr_kneading_stage_steps = job.config.kneading_stage_steps;
            job.curr_circuit = Circuit::from_top_sorted_nodes(
                &top_sorted_nodes,
                &skeleton_graph,
                &gate_map,
                job.config.n as _,
            );

            let (is_correct, diff_indices) = check_probabilisitic_equivalence(
                &job.curr_circuit,
                &original_circuit,
                job.config.probabilitic_eq_check_iterations,
                &mut rng,
            );
            if !is_correct {
                log::error!(
                    "[Error] [Strategy 2] Failed at end of kneading stage. Different at indices {:?}",
                    diff_indices
                );
                assert!(false);
            }

            job.store(&job_path);
        }
    }
}

// This function creates a log4rs configuration for logging to a file.
// It sets up a file appender with a specified path and pattern, and returns the configuration.
fn create_log4rs_config(log_path: &str) -> Result<log4rs::Config, Box<dyn Error>> {
    // Define the file appender with the specified path and pattern
    let file_appender = log4rs::append::file::FileAppender::builder()
        .encoder(Box::new(log4rs::encode::pattern::PatternEncoder::new(
            "{d} - {l} - {m}{n}",
        )))
        .build(log_path)?;

    // Build the configuration
    let config = log4rs::Config::builder()
        .appender(log4rs::config::Appender::builder().build("file", Box::new(file_appender)))
        .build(
            log4rs::config::Root::builder()
                .appender("file")
                .build(log::LevelFilter::Trace),
        )?;

    Ok(config)
}

// This function starts a new obfuscation job or continues an existing one.
// It sets up logging, loads or initializes the job, and runs the appropriate strategy based on the job's configuration.
fn run_obfuscation() {
    let debug = env::var("DEBUG") // only support `DEBUG=true` or `DEBUG=false`
        .ok()
        .and_then(|var| var.parse().ok())
        .unwrap_or(true);

    // Setup logs
    let log_path = args().nth(2).expect("Missing log path");
    let log_confg = create_log4rs_config(&log_path).unwrap();
    log4rs::init_config(log_confg).unwrap();

    let job_path = args().nth(3).expect("Missing obfuscated circuit path");
    let mut job = if std::fs::exists(&job_path).unwrap() {
        log::info!("Found obfuscation job at path. Continuing the pending job.");

        ObfuscationJob::load(&job_path)
    } else {
        log::info!("Starting new obfuscation job at path");
        let orignal_circuit_path = args().nth(4).expect("Missing original circuit path");

        let strategy = args().nth(5).map_or_else(
            || Strategy::Strategy1,
            |sid| match sid.parse::<u8>() {
                Ok(sid) => {
                    if sid == 1 {
                        return Strategy::Strategy1;
                    } else if sid == 2 {
                        return Strategy::Strategy2;
                    } else {
                        assert!(false, "Strategy can either be 1 or 2, not {sid}");
                        return Strategy::Strategy1; // Just to calm the compiler
                    }
                }
                Err(e) => {
                    assert!(false, "Strategy can either be 1 or 2, not {sid}");
                    return Strategy::Strategy1; // Just to calm the compiler
                }
            },
        );

        let config = match strategy {
            Strategy::Strategy1 => ObfuscationConfig::default_strategy1(),
            Strategy::Strategy2 => ObfuscationConfig::default_strategy2(),
        };

        // let (original_circuit, _) =
        // sample_circuit_with_base_gate::<2, u8, _>(300, config.n as u8, 1.0, &mut thread_rng());
        // Circuit::sample_mutli_stage_cipher(config.n, thread_rng());
        let original_circuit = Circuit::sample_multi_stage_cipher(config.n, thread_rng());

        std::fs::write(
            &orignal_circuit_path,
            bincode::serialize(&original_circuit).unwrap(),
        )
        .unwrap();

        ObfuscationJob {
            config,
            curr_total_steps: 0,
            curr_inflationary_stage_steps: 0,
            curr_kneading_stage_steps: 0,
            curr_circuit: original_circuit.clone(),
            original_circuit,
        }
    };

    match job.config.starategy {
        Strategy::Strategy1 => {
            run_strategy1(&mut job, job_path, debug);
        }
        Strategy::Strategy2 => {
            run_strategy2(&mut job, job_path, debug);
        }
    }
}

// This function verifies the correctness of an obfuscation job by checking the functional equivalence of the obfuscated circuit to the original circuit.
fn run_job_verification() {
    let job_path = args().nth(2).expect("Missing obfuscated circuit path");
    std::fs::exists(&job_path).expect("Missing obfuscated circuit at path");
    let job = ObfuscationJob::load(&job_path);

    let iterations = args().nth(3).map_or_else(
        || 1000,
        |id| id.parse::<usize>().map_or_else(|_| 1000, |x| x),
    );

    let original_circuit = &job.original_circuit;
    let obfuscated_circuit = &job.curr_circuit;
    run_verification(original_circuit, obfuscated_circuit, iterations);

    println!("Obfsucated job verification with {iterations} iterations is success");
}

// This function checks whether a file at the given path is a JSON file by examining its extension.
fn is_json_file(file_path: &str) -> bool {
    Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map_or(false, |ext| ext.eq_ignore_ascii_case("json"))
}

// This function checks the functional equivalence of two circuits provided as JSON files.
// It deserializes the circuits, runs a verification process, and prints the result.
fn run_circuits_json_equivalence_check() {
    let c0: Circuit<BaseGate<2, u8>> = {
        let c0_path = args().nth(2).expect("Missing circuit 1 json file at path");
        assert!(is_json_file(&c0_path), "{c0_path} is not circuit JSON file");
        let mut c0_file = std::fs::File::open(c0_path).unwrap();
        let mut c0_contents = String::new();
        c0_file.read_to_string(&mut c0_contents).unwrap();
        let c0: PrettyCircuit = serde_json::from_str(&c0_contents).unwrap();
        (&c0).into()
    };
    let c1: Circuit<BaseGate<2, u8>> = {
        let c1_path = args().nth(3).expect("Missing circuit 2 json file at path");
        assert!(is_json_file(&c1_path), "{c1_path} is not circuit JSON file");
        let mut c1_file = std::fs::File::open(c1_path).unwrap();
        let mut c1_contents = String::new();
        c1_file.read_to_string(&mut c1_contents).unwrap();
        let c0: PrettyCircuit = serde_json::from_str(&c1_contents).unwrap();
        (&c0).into()
    };

    let iterations = args().nth(4).map_or_else(
        || 1000,
        |id| id.parse::<usize>().map_or_else(|_| 1000, |x| x),
    );

    run_verification(&c0, &c1, iterations);

    println!("circuit 0, circuit 1 equivalance check with {iterations} iterations is success");
}

// This function verifies whether two circuits are equivalent by running a probabilistic equivalence check for a specified number of iterations.
fn run_verification(
    c0: &Circuit<BaseGate<2, u8>>,
    c1: &Circuit<BaseGate<2, u8>>,
    iterations: usize,
) {
    let (success, diff_indices) =
        check_probabilisitic_equivalence(c0, c1, iterations, &mut thread_rng());

    if !success {
        println!(
            "Equivalance check failed with following different indices: {:?}",
            diff_indices
        );
    }
}

#[derive(Serialize, Deserialize)]
struct PrettyCircuit {
    wire_count: usize,
    gate_count: usize,
    gates: Vec<[u8; 4]>,
}

impl From<&Circuit<BaseGate<2, u8>>> for PrettyCircuit {
    fn from(circuit: &Circuit<BaseGate<2, u8>>) -> Self {
        PrettyCircuit {
            wire_count: circuit.n(),
            gate_count: circuit.gates().len(),
            gates: circuit
                .gates()
                .iter()
                .map(|gate| {
                    [
                        gate.controls()[0],
                        gate.controls()[1],
                        gate.target(),
                        gate.control_func(),
                    ]
                })
                .collect_vec(),
        }
    }
}

impl From<&PrettyCircuit> for Circuit<BaseGate<2, u8>> {
    fn from(circuit: &PrettyCircuit) -> Self {
        Circuit::new(
            circuit
                .gates
                .iter()
                .enumerate()
                .map(|(id, [control0, control1, target, control_func])| {
                    BaseGate::<2, u8>::new(id, *target, [*control0, *control1], *control_func)
                })
                .collect(),
            circuit.wire_count,
        )
    }
}

// This function converts a binary circuit file to a JSON format.
// It reads the binary file, deserializes the circuit, and writes it to a JSON file.
fn run_convert_circuit_to_json() {
    let input_path = args().nth(2).expect("Missing binary circuit input path");
    let output_path = args().nth(3).expect("[2] Missing json circuit output path");

    let circuit: Circuit<BaseGate<2, u8>> =
        bincode::deserialize(&std::fs::read(input_path).unwrap()).unwrap();

    std::fs::write(
        output_path,
        serde_json::to_string_pretty(&PrettyCircuit::from(&circuit)).unwrap(),
    )
    .unwrap();
}

// This function converts an obfuscation job to a JSON format.
// It loads the job from a binary file and writes the current circuit to a JSON file.
fn run_convert_job_to_json() {
    let input_path = args().nth(2).expect("[1] Missing job input path");
    let output_path = args().nth(3).expect("[2] Missing json circuit output path");

    let job = ObfuscationJob::load(input_path);

    std::fs::write(
        output_path,
        serde_json::to_string_pretty(&PrettyCircuit::from(&job.curr_circuit)).unwrap(),
    )
    .unwrap();
}

// This function evaluates a circuit with a given set of binary inputs.
// It reads the circuit from a JSON file, checks the input length, runs the circuit, and prints the output.
fn run_evaluate_circuit() {
    let circuit_path = args().nth(2).expect("Missing json circuit input path");
    assert!(is_json_file(&circuit_path));
    let inputs = args()
        .nth(3)
        .expect("Missing circuit inputs")
        .split(",")
        .map(|bit| {
            bit.parse::<u8>()
                .ok()
                .and_then(|bit| (bit == 0 || bit == 1).then_some(bit == 1))
                .unwrap_or_else(|| panic!("Expected 0 or 1 but got {}", bit))
        })
        .collect_vec();

    let circuit: &PrettyCircuit =
        &serde_json::from_reader(std::fs::File::open(circuit_path).unwrap()).unwrap();
    let circuit: Circuit<BaseGate<2, u8>> = circuit.into();

    if inputs.len() != circuit.n() {
        panic!(
            "Unexpected number of inputs. Expected {} got {}",
            circuit.n(),
            inputs.len(),
        )
    }

    let mut inputs = inputs;
    circuit.run(&mut inputs);
    println!("{}", inputs.into_iter().map(|bit| bit as u8).join(","))
}

// This is the main function that determines which action to perform based on command-line arguments.
// It can run obfuscation, job verification, circuit conversion, equivalence checks, or circuit evaluation.
fn main() {
    let action = args()
        .nth(1)
        .map_or_else(|| 100, |id| id.parse::<u8>().map_or_else(|_| 100, |x| x));
    match action {
        1 => {
            run_obfuscation();
        }
        2 => {
            run_job_verification();
        }
        3 => {
            run_convert_circuit_to_json();
        }
        4 => {
            run_convert_job_to_json();
        }
        5 => {
            run_circuits_json_equivalence_check();
        }
        6 => {
            run_evaluate_circuit();
        }
        _ => {
            // Help
            println!(
                r#"
            Welcome to Obfustopia

Please refer to README for instructions on how to use.
            
                "#
            );
        }
    }
}
