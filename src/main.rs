use clap::Parser;
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde_json::Value;
use std::time::Instant;
use std::{
    fs::{self, File},
    io::Write,
    process::Command,
};

#[derive(Parser)]
#[command(name = "Check-AV1-Encode")]
#[command(author = "Dennis S.")]
#[command(version = "0.5")]
#[command(about = "Finds the crf needed to make a video ssim2 score 90", long_about = None)]
struct Args {
    /// File to Encode
    #[arg(short = 'i', long)]
    input_file: String,

    /// Encoded File Destination
    #[arg(short = 'o', long)]
    output_file: String,

    /// Encoding Speed
    #[arg(short = 's', long)]
    speed: String,

    /// Amount Of Workers
    #[arg(short = 'w', long)]
    worker_num: String,

    /// Starting Crf
    #[arg(short = 'c', long, default_value_t = 45)]
    crf: i32,

    /// Clip Length in seconds
    #[arg(short = 'l', long, default_value_t = 20)]
    clip_length: i32,

    /// Clip Interval in seconds
    #[arg(short = 'n', long, default_value_t = 360)] //every 6 min
    clip_interval: i32,

    /// select what crf to use on output video (average/smallest)
    #[arg(short = 'u', long, default_value_t = String::from("smallest"))]
    crf_option: String,

    /// if run inside arch wsl enable this
    #[arg(short = 'a', long, default_value_t = false)]
    inside_arch_wsl: bool,
}

fn main() {
    //TODO: multithreading

    let now = Instant::now(); //benchmark until crf found

    let args = Args::parse();
    let input_file = args.input_file;
    let output_file = args.output_file;
    let speed = args.speed;
    let worker_num = args.worker_num;
    let mut current_crf = args.crf;
    let clip_length = args.clip_length;
    let clip_interval = args.clip_interval;
    let crf_used = args.crf_option;

    check_and_create_folders_helpers();

    let json_paths = match get_json() {
        Ok(ok) => ok,
        Err(err) => {
            println!("Err: {}", err);
            let mut line = "".to_string();
            std::io::stdin().read_line(&mut line).unwrap();
            return;
        }
    };
    let av1an_path = json_paths[0].to_string();
    let ssim2_path = json_paths[1].to_string();
    let arch_path = json_paths[2].to_string();
    let ffmpeg_path = json_paths[3].to_string();
    let ffprobe_path = json_paths[4].to_string();
    let av1an_setings_unformatted = json_paths[5].to_string();

    //get clips
    let clip_names = extract_clips(
        &input_file,
        clip_length,
        clip_interval,
        &ffmpeg_path,
        &ffprobe_path,
    )
    .unwrap();

    if clip_names[0] == input_file {
        println!("Clip_Length is bigger then the whole video, please check the settings");
        let mut line = "".to_string();
        std::io::stdin().read_line(&mut line).unwrap();
        return;
    }

    let total_cpu_threads = std::thread::available_parallelism()
        .expect("Failed retrieving number of threads")
        .get();
    let workers: usize = worker_num
        .parse()
        .expect("Failed parsing number of workers");
    let num_of_threads = total_cpu_threads / workers;
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_of_threads)
        .build_global()
        .unwrap(); // Sets the threads used by rayon's internal ThreadPool

    let _num_of_clips = clip_names.len();

    //for each clip find crf
    let crf_values: Vec<i32> = clip_names
        .par_iter()
        .map(|clip_name| {
            find_crf_for_90_ssim2(
                current_crf,
                clip_name,
                &av1an_setings_unformatted,
                &speed,
                &worker_num,
                &av1an_path,
                &arch_path,
                &ssim2_path,
                &"0".to_string(),
            )
        })
        .collect();
    // Par iter creates an parallel iterator using the global threads defined previously
    // Then map returns the value of the function find_crf_for_90_ssim2
    // Finally we use collect(), which automatically parses the collection generated by the iterator and stores it as an i32

    let min_crf = find_lowest_crf(crf_values.clone());
    let average_crf = find_average_crf(crf_values.clone());
    let crf_used_use;
    if crf_used == "smallest".to_string() {
        crf_used_use = min_crf;
    } else {
        crf_used_use = average_crf;
    }
    println!("{:?}", crf_values);
    println!("min_crf: {}", min_crf);
    println!("average_crf: {}", average_crf);
    println!("TIME_ELAPSED: {}", now.elapsed().as_secs());
    let av1an_settings = format_encoding_settings(
        &av1an_setings_unformatted,
        &input_file,
        &speed,
        &crf_used_use.to_string(),
        &worker_num,
        &output_file,
    );
    encode_clip(&input_file, &av1an_path, &av1an_settings).unwrap();
    println!("Finished Encoding: {}", input_file);
}

fn encode_clip(
    clip_path: &String,
    av1an_path: &String,
    av1an_settings: &String,
) -> Result<i32, String> {
    //
    //  start encoding a clip with crf given and additional settings
    //
    let file_name = "av1an_encode_settings.bat".to_string();
    //try to create a file to encode with
    match create_file_encoding_settings(&av1an_settings, &file_name) {
        Ok(ok) => ok,
        Err(_err) => {
            let error_messege = "Cannot Create File".to_string();
            return Err(error_messege);
        }
    };
    //try start encoding
    let av1an_args = ["/C", &file_name, &av1an_path];
    let av1an_error = format!("Cannot start encoding file: {}\nError: ", clip_path);
    spawn_a_process(av1an_args, &av1an_error).unwrap();
    return Ok(0);
}

fn ssim2_clip(
    original_clip_path: &String,
    encoded_clip_path: &String,
    arch_path: &String,
    ssim2_path: &String,
    worker_num: &String,
    thread: &String,
) -> Result<Vec<String>, String> {
    //run ssmi2 with arch wsl
    //return 95th percentile and 5th percentile if succeeded
    let mut results_vec: Vec<String> = Vec::new();

    let save_file_name = format!("output_helper/ssim2/ssim2_output_{}.txt", thread);

    let ssmi2_settings = format!(
        "%1 runp {} video -f {} \"{}\" \"{}\" > {}",
        ssim2_path, worker_num, original_clip_path, encoded_clip_path, save_file_name
    );

    let file_name = "ssmi2_encode_settings.bat".to_string();
    match create_file_encoding_settings(&ssmi2_settings, &file_name) {
        Ok(ok) => ok,
        Err(_err) => {
            let error_messege = "Cannot Create File".to_string();
            return Err(error_messege);
        }
    };

    //try start ssim2
    let ssim2_args = ["/C", &file_name, &arch_path];
    let ssim2_error = format!("While Trying to ssim2 clip: {}\nError: ", encoded_clip_path);
    spawn_a_process(ssim2_args, &ssim2_error).unwrap();

    let output_file_content =
        fs::read_to_string(save_file_name).expect("Should have been able to read ssim2_output.txt");
    let lines: Vec<&str> = output_file_content.split("\n").collect();
    let pre_last_line = lines[lines.len() - 2]; //last line is empty
    let first_colon_index = pre_last_line.find(":").unwrap();
    let first_dot_index = pre_last_line.find(".").unwrap();
    let ninty_fifth_percent_in_str = pre_last_line
        .get((first_colon_index + 2)..first_dot_index)
        .unwrap();
    //let ninty_fifth_percent: i32 = ninty_fifth_percent_in_str.parse().unwrap();

    results_vec.push(ninty_fifth_percent_in_str.to_string());
    return Ok(results_vec);
}

fn create_file_encoding_settings(settings: &String, file_name: &String) -> Result<String, i32> {
    //
    //  writes a batch file to encode with later
    //  this is becuase procces in rust use string leterals or something :(
    //
    let mut file = match File::create(file_name) {
        Ok(ok) => ok,
        Err(_err) => return Err(3),
    };
    match file.write_all(settings.as_bytes()) {
        Ok(_ok) => return Ok("Success".to_string()),
        Err(_err) => return Err(3),
    };
}

fn get_json() -> Result<Vec<String>, String> {
    //
    //  get paths for programs with json
    //
    let mut final_vec: Vec<String> = Vec::new();
    let json_file_string = match fs::read_to_string("paths.json") {
        Ok(string) => string,
        Err(_err) => {
            let error_messege = "Cannot Open/find json file \"paths.json\"".to_string();
            return Err(error_messege);
        }
    };
    //whole json
    let json_values: Value = match serde_json::from_str(&json_file_string) {
        Ok(value) => value,
        Err(_err) => {
            let error_messege = "\"paths.json\" fromatted incorectly".to_string();
            return Err(error_messege);
        }
    };

    //paths inside json
    let av1an_path_value = match json_values["av1an"].as_str() {
        Some(str) => str.to_string(),
        None => {
            let error_messege = "\"av1an\" was not found inside \"paths.json\"".to_string();
            return Err(error_messege);
        }
    };
    let ssim2_path_value = match json_values["ssim2"].as_str() {
        Some(str) => str.to_string(),
        None => {
            let error_messege = "\"ssim2\" was not found inside \"paths.json\"".to_string();
            return Err(error_messege);
        }
    };
    let arch_path_value = match json_values["arch"].as_str() {
        Some(str) => str.to_string(),
        None => {
            let error_messege = "\"arch\" was not found inside \"paths.json\"".to_string();
            return Err(error_messege);
        }
    };
    let ffmpeg_path_value = match json_values["ffmpeg"].as_str() {
        Some(str) => str.to_string(),
        None => {
            let error_messege = "\"ffmpeg\" was not found inside \"paths.json\"".to_string();
            return Err(error_messege);
        }
    };
    let ffprobe_path_value = match json_values["ffprobe"].as_str() {
        Some(str) => str.to_string(),
        None => {
            let error_messege = "\"ffprobe\" was not found inside \"paths.json\"".to_string();
            return Err(error_messege);
        }
    };
    let av1an_settings_path_value = match json_values["encoding_settings"].as_str() {
        Some(str) => str.to_string(),
        None => {
            let error_messege =
                "\"encoding_settings\" was not found inside \"paths.json\"".to_string();
            return Err(error_messege);
        }
    };
    final_vec.push(av1an_path_value);
    final_vec.push(ssim2_path_value);
    final_vec.push(arch_path_value);
    final_vec.push(ffmpeg_path_value);
    final_vec.push(ffprobe_path_value);
    final_vec.push(av1an_settings_path_value);
    return Ok(final_vec);
}

fn extract_clips(
    full_video: &String,
    clip_length: i32,
    interval: i32,
    ffmpeg_path: &String,
    ffprobe_path: &String,
) -> Result<Vec<String>, String> {
    //
    //  first get the video length using ffprobe
    //  then in a for loop extract each clip using the clip_length and the interval
    //  last return all the clip names in a vec
    //
    let mut final_vec: Vec<String> = Vec::new();

    let file_name_ffprobe = "ffprobe_settings.bat".to_string();
    let file_name_ffmpeg = "ffmpeg_settings.bat".to_string();
    let file_name_ffprobe_output = "output_helper/ffprobe/ffprobe_output.txt";
    //ffprobe -v error -select_streams v:0 -show_entries stream=duration -of default=noprint_wrappers=1:nokey=1 "/mnt/c/Encode/720p_15s.mp4"
    let ffprobe_settings = format!("%1 -v error -select_streams v:0 -show_entries stream=duration -of default=noprint_wrappers=1:nokey=1 \"{}\" > {}",
        full_video, file_name_ffprobe_output);

    //try to create a file to encode with
    match create_file_encoding_settings(&ffprobe_settings, &file_name_ffprobe) {
        Ok(ok) => ok,
        Err(_err) => {
            let error_messege = "Cannot Create File".to_string();
            return Err(error_messege);
        }
    };
    let ffprobe_args = ["/C", &file_name_ffprobe, &ffprobe_path];
    let ffprobe_error = format!("Cannot probe file: {}\nError: ", full_video);
    output_a_process(ffprobe_args, &ffprobe_error).unwrap();

    //read the result that was saved to a file
    let output_file_content = fs::read_to_string(file_name_ffprobe_output)
        .expect("Should have been able to read ffprobe_output.txt");
    let first_dot_index = output_file_content.find(".").unwrap();
    let video_length_in_str = output_file_content.get(0..first_dot_index).unwrap();
    let video_length: i32 = video_length_in_str.parse().unwrap();
    if video_length < clip_length {
        //dont make clip just tell the av1an to encode the whole video
        //as it is really small, smaller then the clip that the user wanted
        final_vec.push(full_video.to_string());
        return Ok(final_vec);
    }
    let mut length_passed = 0;
    let mut current_file_name_index = 0;
    while length_passed < video_length {
        let current_file_name = length_passed.to_string()
            + &"-".to_string()
            + &(length_passed + clip_length).to_string()
            + &"-".to_string()
            + &current_file_name_index.to_string()
            + ".mkv";
        let ffmpeg_settings = format!(
            "%1 -ss {} -i \"{}\" -c copy -t {} \"output_helper/clips/{}\"",
            length_passed, full_video, clip_length, current_file_name
        );

        match create_file_encoding_settings(&ffmpeg_settings, &file_name_ffmpeg) {
            Ok(ok) => ok,
            Err(_err) => {
                let error_messege = "Cannot Create File".to_string();
                return Err(error_messege);
            }
        };

        let ffmpeg_args = ["/C", &file_name_ffmpeg, &ffmpeg_path];
        let ffmpeg_error = format!("Cannot clip file: {}\nError: ", full_video);
        output_a_process(ffmpeg_args, &ffmpeg_error).unwrap();
        final_vec.push(current_file_name);

        length_passed += clip_length + interval;
        current_file_name_index += 1;
    }
    println!("Created all the Clips");
    return Ok(final_vec);
}

fn format_encoding_settings(
    settings: &String,
    input_file: &String,
    speed: &String,
    crf: &String,
    worker_num: &String,
    output_file: &String,
) -> String {
    let mut final_string = settings.clone();
    final_string = final_string.replace(
        "INPUT",
        &("\"".to_string() + input_file + &"\"".to_string()),
    ); //INPUT
    final_string = final_string.replace("SPEED", speed); //SPEED
    final_string = final_string.replace("CRF", crf); //CRF/QUANTIZER
    final_string = final_string.replace("WORKER_NUM", worker_num); //WORKER_NUM
    final_string = final_string.replace(
        "OUTPUT",
        &("\"".to_string() + output_file + &"\"".to_string()),
    ); //OUTPUT
    return final_string;
}

fn check_and_create_folders_helpers() {
    //delete latest encode
    fs::remove_dir_all("output_helper/").unwrap();
    fs::create_dir_all("output_helper/ssim2").unwrap();
    fs::create_dir_all("output_helper/clips").unwrap();
    fs::create_dir_all("output_helper/clips_encoded").unwrap();
    fs::create_dir_all("output_helper/ffprobe").unwrap();
}

fn spawn_a_process(args: [&str; 3], custom_error: &String) -> Result<i32, String> {
    //using spawn to show the user the program running
    let process = match Command::new("cmd").args(args).spawn() {
        Ok(out) => out,
        Err(err) => return Err(custom_error.to_string() + &err.to_string()),
    };

    let output = match process.wait_with_output() {
        Ok(ok) => ok,
        Err(err) => return Err(custom_error.to_string() + &err.to_string()),
    };

    if output.status.success() {
        return Ok(1);
    } else {
        return Err(custom_error.to_string()
            + &"no error, some program start and finished without doing anything".to_string());
    }
}

fn output_a_process(args: [&str; 3], custom_error: &String) -> Result<i32, String> {
    //using output to hide the program
    let process = match Command::new("cmd").args(args).output() {
        Ok(out) => out,
        Err(err) => return Err(custom_error.to_string() + &err.to_string()),
    };

    if process.status.success() {
        return Ok(1);
    } else {
        return Err(custom_error.to_string()
            + &"no error, some program start and finished without doing anything".to_string());
    }
}

fn find_crf_for_90_ssim2(
    starting_crf: i32,
    clip_name: &String,
    av1an_setings_unformatted: &String,
    speed: &String,
    worker_num: &String,
    av1an_path: &String,
    arch_path: &String,
    ssim2_path: &String,
    thread: &String,
) -> i32 {
    let mut current_crf = starting_crf;
    let ssmi2_check_valid = false;
    let current_clip_name = format!("output_helper/clips/{}", clip_name);
    let current_clip_encoded_name = format!("output_helper/clips_encoded/{}", clip_name);
    let mut was_above_90 = false;
    let mut was_below_90 = false;
    while !ssmi2_check_valid {
        let current_crf_str: String = current_crf.to_string();
        let av1an_settings = format_encoding_settings(
            av1an_setings_unformatted,
            &current_clip_name,
            speed,
            &current_crf_str,
            worker_num,
            &current_clip_encoded_name,
        );
        encode_clip(&current_clip_name, av1an_path, &av1an_settings).unwrap();
        let ssim2_results = ssim2_clip(
            &current_clip_name,
            &current_clip_encoded_name,
            arch_path,
            ssim2_path,
            &worker_num.to_string(),
            thread,
        )
        .unwrap();
        let result_95: i32 = ssim2_results[0].parse().unwrap();
        println!(
            "\n\n\n\ncurrent_clip: {}, current_crf: {}, current_ssim2: {}",
            current_clip_name, current_crf, ssim2_results[0]
        );
        if result_95 == 90 {
            return current_crf;
            //found the crf wanted
        }
        if result_95 < 90 {
            if was_above_90 {
                was_below_90 = true;
                current_crf -= 1;
            } else {
                current_crf -= 5;
            }
        }
        if result_95 > 90 {
            was_above_90 = true;
            if was_below_90 {
                current_crf += 1;
            } else {
                current_crf += 5;
            }
        }
        fs::remove_file(current_clip_encoded_name.to_string()).unwrap(); //delete encoded file to encode again
        fs::remove_file(current_clip_encoded_name.to_string() + &".lwi".to_string()).unwrap();
        //delete encoded file iwi for ssim2
    }

    return -1;
}

fn find_lowest_crf(crf_list: Vec<i32>) -> i32 {
    crf_list
        .iter()
        .min()
        .expect("Failed getting the minimum crf")
        .to_owned()
}

fn find_average_crf(crf_list: Vec<i32>) -> i32 {
    let list_len = crf_list.len();
    let sum: i32 = crf_list.iter().sum();

    return sum / (list_len as i32);
}
