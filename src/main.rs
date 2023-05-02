extern crate env_logger;
extern crate librespot_audio;
extern crate librespot_core;
extern crate librespot_metadata;
#[macro_use]
extern crate log;
extern crate regex;
extern crate scoped_threadpool;
extern crate tokio;

use std::env;
use std::{thread, time};
use std::convert::TryInto;
use std::io::{self, BufRead, Read, Result};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use env_logger::{Builder, Env};
use librespot_audio::{AudioDecrypt, AudioFile};
use librespot_core::authentication::Credentials;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_core::spotify_id::SpotifyId;
use librespot_metadata::{Artist, FileFormat, Metadata, Track, Album};
use regex::Regex;
use scoped_threadpool::Pool;
use tokio::runtime::Runtime;

fn main() {
    Builder::from_env(Env::default().default_filter_or("info")).init();

    let args: Vec<_> = env::args().collect();
    assert!(args.len() == 3 || args.len() == 4, "Usage: {} user password [helper_script] < tracks_file", args[0]);

    let core = Runtime::new().unwrap();
    let session_config = SessionConfig::default();
    let credentials = Credentials::with_password(args[1].to_owned(), args[2].to_owned());
    info!("Connecting ...");
    let session = core
        .block_on(Session::connect(session_config, credentials, None, false))
        .unwrap().0;
    info!("Connected!");

    let mut threadpool = Pool::new(1);

    let spotify_uri = Regex::new(r"^spotify:track:([[:alnum:]]{22})$").unwrap();
    let spotify_url = Regex::new(r"^open\.spotify\.com/track/([[:alnum:]]{22})$").unwrap();
    let spotify_id = Regex::new(r"^([[:alnum:]]{22})$").unwrap();

    io::stdin().lock().lines()
        .filter_map(|line|
            line.ok().and_then(|str|
                spotify_uri.captures(&str).or(spotify_url.captures(&str))
                    .or(spotify_id.captures(&str))
                    .or_else(|| { warn!("Cannot parse track from string {}", str); None })
                    .and_then(|capture|SpotifyId::from_base62(&capture[1]).ok())))
        .for_each(|id|{
            info!("Getting track {}...", id.to_base62().expect("UTF8 error"));
            let mut track = core.block_on(Track::get(&session, id)).expect("Cannot get track metadata");
            let artists_strs: Vec<_> = track.artists.iter().map(|id|core.block_on(Artist::get(&session, *id)).expect("Cannot get artist metadata").name).collect();
            let album = core.block_on(Album::get(&session, track.album)).expect("Cannot get album metadata");
            let track_name = track.name.to_string();

            // from
            // https://stackoverflow.com/questions/38461429/how-can-i-truncate-a-string-to-have-at-most-n-characters

            /*
            let max_width = 255-4;
            let fname_minus_extension = format!("{} --- {} --- {} --- {}", id.to_base62().expect("UTF8 error"), artists_strs.join(", "), track_name, album.name)
                .chars()
                .take(max_width)
                .collect::<String>()
                .replace("/"," ");
            let fname = format!("{}.ogg",fname_minus_extension);
            */
            let fname = format!("{}.ogg",id.to_base62().expect("UTF8 error"));

            if Path::new(&fname).exists() {
                info!("{} - is already downloaded", fname);
                return;
            }


            if !track.available {
                warn!("Track {} is not available, finding alternative...", id.to_base62().expect("UTF8 error"));
                let alt_track = track.alternatives.iter().find_map(|id|{
                    let alt_track = core.block_on(Track::get(&session, *id)).expect("Cannot get track metadata");
                    match alt_track.available {
                        true => {
                            if alt_track.id.to_base62().expect("UTF8 error") == track.id.to_base62().expect("UTF8 error") {
                                //warn!("ALTERNATE ID SAME");
                                return None;
                            }
                            return Some(alt_track);
                        }
                        false => None
                    }
                });
                match alt_track {
                    Some(alt_track) => {
                        warn!("Found track alternative {} -> {}", id.to_base62().expect("UTF8 error"), alt_track.id.to_base62().expect("UTF8 error"));
                        track = alt_track;
                    }
                    None => {
                        warn!("Could not find alternative for track {}", id.to_base62().expect("UTF8 error"));
                        return;
                    }
                 }
            }

            let stopwatch = time::Instant::now();

            debug!("File formats: {}", track.files.keys().map(|filetype|format!("{:?}", filetype)).collect::<Vec<_>>().join(" "));
            let file_id = track.files.get(&FileFormat::OGG_VORBIS_320)
                .or(track.files.get(&FileFormat::OGG_VORBIS_160))
                .or(track.files.get(&FileFormat::OGG_VORBIS_96))
                .expect("Could not find a OGG_VORBIS format for the track.");
            let key = core.block_on(session.audio_key().request(track.id, *file_id)).expect("Cannot get audio key");
            let mut encrypted_file = core.block_on(AudioFile::open(&session, *file_id, 320, true)).unwrap();
            let mut buffer = Vec::new();
            let mut read_all: Result<usize> = Ok(0);
            threadpool.scoped(|scope|{
                scope.execute(||{
                    read_all = encrypted_file.read_to_end(&mut buffer);
                });
            });
            read_all.expect("Cannot read file stream");
            let mut decrypted_buffer = Vec::new();
            AudioDecrypt::new(key, &buffer[..]).read_to_end(&mut decrypted_buffer).expect("Cannot decrypt stream");
            if args.len() == 3 {
                std::fs::write(&fname, &decrypted_buffer[0xa7..]).expect("Cannot write decrypted track");
                info!("Filename: {}", fname);
            } else {
                let mut cmd = Command::new(args[3].to_owned());
                cmd.stdin(Stdio::piped());
                cmd.arg(id.to_base62().expect("UTF8 error")).arg(track_name).arg(album.name).args(artists_strs.iter());
                let mut child = cmd.spawn().expect("Could not run helper program");
                let pipe = child.stdin.as_mut().expect("Could not open helper stdin");
                pipe.write_all(&decrypted_buffer[0xa7..]).expect("Failed to write to stdin");
                assert!(child.wait().expect("Out of ideas for error messages").success(), "Helper script returned an error");
            }

            let track_length = time::Duration::from_millis(track.duration.try_into().unwrap());
            let sleep_for = track_length.saturating_sub(stopwatch.elapsed());

            if !sleep_for.is_zero() {
                info!("Sleeping for {}.{:0>3} seconds per rate limit ...", sleep_for.as_secs(), sleep_for.subsec_millis());
                thread::sleep(sleep_for);
            }
        });
}
