#[macro_use]
extern crate clap;

use clap::Arg;
use log::{error, info};
use quick_error::quick_error;
use std::error::Error;
use std::fs::File;
use std::io;
use std::io::Write;
use std::io::{BufRead, BufReader, BufWriter};
use std::num::ParseFloatError;
use std::num::ParseIntError;
use std::path::Path;
use std::process::{exit, Command, Stdio};
use std::{fs, sync};
use threadpool::ThreadPool;

quick_error! {
    #[derive(Debug)]
    pub enum ViewerError {
        /// IO Error
        Io(err: io::Error) {
            from()
            cause(err)
        }
        ParseInt(err: ParseIntError) {
            from()
            cause(err)
            description("failed to parse int number")
            display(self_) -> ("{}: {}", self_.description(), err)
        }
        ParseFloat(err: ParseFloatError) {
            from()
            cause(err)
            description("failed to parse float number")
            display(self_) -> ("{}: {}", self_.description(), err)
        }
        Other(s: &'static str) {
            display(self_) -> ("{}", s)
        }
    }
}

fn main() -> Result<(), ViewerError> {
    env_logger::init();

    const EXIT_FAILURE: i32 = 1;

    let matches = app_from_crate!()
        .arg(
            Arg::with_name("path")
                .default_value("./n-body-output")
                .help("Sets n body output path")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("size")
                .long("size")
                .short("s")
                .default_value("1920,1080")
                .help("Sets video size")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("frame-rate")
                .long("frame-rate")
                .short("f")
                .default_value("30")
                .help("Sets frame rate of video")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("point-type")
                .long("point-type")
                .short("p")
                .default_value("1")
                .help("Sets point type of gnu plot")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("worker")
                .long("worker")
                .short("w")
                .help("Sets worker number")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("initial-rotation")
                .long("initial-rotation")
                .help("Sets initial rotation degree")
                .default_value("45")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("rotation-speed")
                .long("rotation-speed")
                .help("Sets the rotation speed(degree per frame)")
                .default_value("0.1")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("min-bounds")
                .long("min-bounds")
                .help("Force set min bounds")
                .takes_value(true)
                .requires("max-bounds"),
        )
        .arg(
            Arg::with_name("max-bounds")
                .long("max-bounds")
                .help("Force set max bounds")
                .takes_value(true)
                .requires("min-bounds"),
        )
        .get_matches();
    info!("{:?}", matches);
    let path = matches.value_of("path").unwrap();
    let directory = Path::new(&path);
    let size = matches.value_of("size").unwrap();
    let point_type = matches.value_of("point-type").unwrap();
    let initial_rotation: f64 = matches.value_of("initial-rotation").unwrap().parse()?;
    let rotation_speed: f64 = matches.value_of("rotation-speed").unwrap().parse()?;
    let worker_num = match matches.value_of("worker") {
        Some(w) => w.parse()?,
        None => num_cpus::get(),
    };
    let frame_rate = matches.value_of("frame-rate").unwrap();
    if directory.is_dir() {
        let sample_number: usize = fs::read_to_string(directory.join("_sample.txt"))?
            .trim()
            .parse()?;
        info!("sample number: {}", sample_number);
        let sample_time: f64 = fs::read_to_string(directory.join("_time.txt"))?
            .trim()
            .parse()?;
        info!("sample time: {} s", sample_time);
        let (min_bounds, max_bounds) = if matches.value_of("min-bounds").is_some() {
            let min_bounds: Vec<f64> = read_bounds(matches.value_of("min-bounds").unwrap())?;
            let max_bounds: Vec<f64> = read_bounds(matches.value_of("max-bounds").unwrap())?;
            (min_bounds, max_bounds)
        } else {
            let bounds = File::open(directory.join("_bounds.dat"))?;
            let mut bounds = BufReader::new(bounds)
                .lines()
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|line| {
                    let line = line.trim();
                    !line.is_empty() && !line.starts_with('#')
                });
            let min_bounds: Vec<f64> = read_bounds(
                &bounds
                    .next()
                    .ok_or(ViewerError::Other("min bounds line missing"))?,
            )?;
            let max_bounds: Vec<f64> = read_bounds(
                &bounds
                    .next()
                    .ok_or(ViewerError::Other("max bounds line missing"))?,
            )?;
            (min_bounds, max_bounds)
        };
        assert_eq!(max_bounds.len(), min_bounds.len());
        let dimension = max_bounds.len();
        assert!(dimension == 2 || dimension == 3);

        let pool = ThreadPool::new(worker_num);
        let (tx, rx) = sync::mpsc::channel::<Result<(usize, Option<i32>), ViewerError>>(); // create a channel for counting
        let job_number = sample_number + 1; // from 0 to sample_number
        for i in 0..=sample_number {
            let tx = tx.clone();
            let directory = directory.to_owned();
            let size = size.to_owned();
            let point_type = point_type.to_owned();
            let min_bounds = min_bounds.clone();
            let max_bounds = max_bounds.clone();
            pool.execute(move || {
                tx.send((move || -> Result<_, ViewerError> {
                    let child = {
                        let time_point = sample_time * i as f64;
                        let input_path = directory.join(format!("{}.dat", i));
                        let output_path = directory.join(format!("{}.png", i));
                        let title = format!("time = {:.19} s", time_point);

                        let mut gnuplot = Command::new("gnuplot")
                            .stdin(Stdio::piped())
                            .stdout(Stdio::inherit())
                            .stderr(Stdio::inherit())
                            .spawn()?;
                        {
                            let gnuplot_stdin =
                                gnuplot.stdin.as_mut().expect("failed to get piped stdin");
                            let mut writer = BufWriter::new(gnuplot_stdin);
                            writeln!(
                                writer,
                                "set terminal pngcairo size {} enhanced font 'Verdana,10'",
                                size
                            )?;
                            writeln!(writer, "set view equal xyz")?;
                            writeln!(writer, "set xyplane relative 0")?;
                            writeln!(writer, "set output {:?}", output_path)?;
                            writeln!(writer, "set view 60,{}", (initial_rotation + i as f64 * rotation_speed) % 360f64)?;
                            if dimension == 2 {
                                write!(writer, "plot ")?;
                            } else {
                                assert_eq!(dimension, 3);
                                write!(writer, "splot ")?;
                            }
                            // write bounds
                            for d in 0..dimension {
                                write!(writer, "[{}:{}] ", min_bounds[d], max_bounds[d])?;
                            }
                            writeln!(
                                writer,
                                "{:?} title '{}' pointtype {}",
                                input_path, title, point_type
                            )?;
                        }
                        gnuplot
                    };
                    let output = child.wait_with_output()?;
                    Ok((i, output.status.code()))
                })())
                    .expect("failed to send item through channel tx");
            });
        }

        let finished =
            rx.iter()
                .take(job_number)
                .fold(Ok(0), |num: Result<usize, ViewerError>, result| {
                    let (i, status) = result?;
                    println!("child {} finished with status {:?}", i, status);
                    Ok(num? + 1usize)
                })?;
        assert_eq!(finished, job_number);

        let child = {
            let input_pattern = directory.join("%d.png");
            let output_path = directory.join("_video.mp4");

            Command::new("ffmpeg")
                .arg("-y")
                .arg("-r")
                .arg(frame_rate)
                .arg("-i")
                .arg(input_pattern)
                .args(&["-c:v", "libx264"])
                .arg(output_path)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?
        };
        let output = child.wait_with_output()?;
        println!(
            "video creation child process exited with status {:?}",
            output.status.code()
        );
        Ok(())
    } else {
        error!("{:?} is not a directory", path);
        exit(EXIT_FAILURE)
    }
}

fn read_bounds(s: &str) -> Result<Vec<f64>, ParseFloatError> {
    s.split(' ')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .collect()
}
