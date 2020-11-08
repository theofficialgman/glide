use directories::ProjectDirs;
use gio::prelude::*;
use glib::ToVariant;
use std::boxed::Box;
use std::cell::RefCell;
use std::fs::create_dir_all;
use std::{thread, time};

use crate::app;

#[cfg(target_os = "macos")]
use fruitbasket::FruitApp;
#[cfg(target_os = "macos")]
use fruitbasket::RunPeriod;

use crate::channel_player::{
    register_player, AudioVisualization, ChannelPlayer, PlaybackState, PlayerEvent, SeekDirection, SubtitleTrack,
};
use crate::video_renderer::VideoRenderer;

use gst_player::PlayerStreamInfoExt;

#[derive(Serialize, Deserialize)]
enum UIAction {
    ForwardedPlayerEvent(PlayerEvent),
    Quit,
}

pub struct VideoPlayer {
    pub player: ChannelPlayer,
    pub app: Box<app::Application>,
    fullscreen_action: gio::SimpleAction,
    restore_action: gio::SimpleAction,
    pause_action: gio::SimpleAction,
    seek_forward_action: gio::SimpleAction,
    seek_backward_action: gio::SimpleAction,
    subtitle_action: gio::SimpleAction,
    audio_visualization_action: gio::SimpleAction,
    audio_track_action: gio::SimpleAction,
    video_track_action: gio::SimpleAction,
    open_media_action: gio::SimpleAction,
    open_subtitle_file_action: gio::SimpleAction,
    audio_mute_action: gio::SimpleAction,
    volume_increase_action: gio::SimpleAction,
    volume_decrease_action: gio::SimpleAction,
    dump_pipeline_action: gio::SimpleAction,
    sender: channel::Sender<UIAction>,
    receiver: channel::Receiver<UIAction>,
    player_receiver: Option<channel::Receiver<PlayerEvent>>,
}

thread_local!(
    pub static GLOBAL: RefCell<Option<VideoPlayer>> = RefCell::new(None)
);

#[macro_export]
macro_rules! with_video_player {
    ($player:ident $code: block) => (
        GLOBAL.with(|global| {
            if let Some(ref $player) = *global.borrow() $code
        })
    )
}

#[macro_export]
macro_rules! with_mut_video_player {
    ($player:ident $code: block) => (
        GLOBAL.with(|global| {
            if let Some(ref mut $player) = *global.borrow_mut() $code
        })
    )
}

pub fn register_player_and_run(mut video_player: VideoPlayer, args: &Vec<std::string::String>) {
    #[cfg(target_os = "macos")]
    video_player.start();

    let app = &mut video_player.app;

    #[cfg(target_os = "macos")]
    app.start();

    app.set_args(args);

    let implementation = app.implementation();
    GLOBAL.with(move |global| {
        *global.borrow_mut() = Some(video_player);
        if let Some(ref mut player) = *global.borrow_mut() {
            player.post_init();
        }
    });

    match implementation {
        Some(implementation) => match implementation {
            #[cfg(target_os = "linux")]
            app::ApplicationImpl::GTK(gtk_app) => {
                gtk_app.run(args);
            }
            #[cfg(target_os = "macos")]
            app::ApplicationImpl::Cocoa(mut fruit_app) => {
                let _ = fruit_app.run(RunPeriod::Forever);
            }
        },
        None => {
            eprintln!("No UI implementation found");
        }
    }
}

// Only possible in nightly
// static SEEK_BACKWARD_OFFSET: gst::ClockTime = gst::ClockTime::from_mseconds(2000);
// static SEEK_FORWARD_OFFSET: gst::ClockTime = gst::ClockTime::from_mseconds(5000);

static SEEK_BACKWARD_OFFSET: gst::ClockTime = gst::ClockTime(Some(2_000_000_000));
static SEEK_FORWARD_OFFSET: gst::ClockTime = gst::ClockTime(Some(5_000_000_000));

fn ui_action_handle() -> glib::Continue {
    eprintln!("ui_action_handle");
    with_video_player!(player {
        if let Ok(action) = &player.receiver.try_recv() {
            match action {
                UIAction::Quit => {
                    player.quit();
                }
                UIAction::ForwardedPlayerEvent(event) => {
                    player.dispatch_event(event);
                }
            }
        }
    });
    glib::Continue(false)
}

impl VideoPlayer {
    pub fn new(glide_app: Box<app::Application>, video_renderer: Box<VideoRenderer>) -> Self {
        let fullscreen_action = gio::SimpleAction::new_stateful("fullscreen", None, &false.to_variant());
        glide_app.add_action(&fullscreen_action);

        let restore_action = gio::SimpleAction::new_stateful("restore", None, &true.to_variant());
        glide_app.add_action(&restore_action);

        let pause_action = gio::SimpleAction::new_stateful("pause", None, &false.to_variant());
        glide_app.add_action(&pause_action);

        let seek_forward_action = gio::SimpleAction::new_stateful("seek-forward", None, &false.to_variant());
        glide_app.add_action(&seek_forward_action);

        let seek_backward_action = gio::SimpleAction::new_stateful("seek-backward", None, &false.to_variant());
        glide_app.add_action(&seek_backward_action);

        let open_media_action = gio::SimpleAction::new("open-media", None);
        glide_app.add_action(&open_media_action);

        let open_subtitle_file_action = gio::SimpleAction::new("open-subtitle-file", None);
        glide_app.add_action(&open_subtitle_file_action);

        let audio_mute_action = gio::SimpleAction::new_stateful("audio-mute", None, &false.to_variant());
        glide_app.add_action(&audio_mute_action);

        let volume_increase_action =
            gio::SimpleAction::new_stateful("audio-volume-increase", None, &false.to_variant());
        glide_app.add_action(&volume_increase_action);

        let volume_decrease_action =
            gio::SimpleAction::new_stateful("audio-volume-decrease", None, &false.to_variant());
        glide_app.add_action(&volume_decrease_action);

        let dump_pipeline_action = gio::SimpleAction::new_stateful("dump-pipeline", None, &false.to_variant());
        glide_app.add_action(&dump_pipeline_action);

        let subtitle_action =
            gio::SimpleAction::new_stateful("subtitle", glib::VariantTy::new("s").ok(), &"".to_variant());
        glide_app.add_action(&subtitle_action);

        let audio_visualization_action = gio::SimpleAction::new_stateful(
            "audio-visualization",
            glib::VariantTy::new("s").ok(),
            &"none".to_variant(),
        );
        glide_app.add_action(&audio_visualization_action);

        let audio_track_action =
            gio::SimpleAction::new_stateful("audio-track", glib::VariantTy::new("s").ok(), &"audio-0".to_variant());
        glide_app.add_action(&audio_track_action);

        let video_track_action =
            gio::SimpleAction::new_stateful("video-track", glib::VariantTy::new("s").ok(), &"video-0".to_variant());
        glide_app.add_action(&video_track_action);

        let about = gio::SimpleAction::new("about", None);
        about.connect_activate(move |_, _| {
            with_video_player!(video_player {
                video_player.app.display_about_dialog();
            });
        });
        glide_app.add_action(&about);

        let quit = gio::SimpleAction::new("quit", None);
        quit.connect_activate(|_, _| {
            with_video_player!(video_player {
                video_player.quit();
            });
        });
        glide_app.add_action(&quit);

        let (sender, receiver) = channel::unbounded();

        let app_context = glide_app.glib_context();
        let player = ChannelPlayer::new(video_renderer, app_context);

        let video_player = Self {
            player,
            app: glide_app,
            fullscreen_action,
            restore_action,
            pause_action,
            seek_forward_action,
            seek_backward_action,
            subtitle_action,
            audio_visualization_action,
            audio_track_action,
            video_track_action,
            open_media_action,
            open_subtitle_file_action,
            audio_mute_action,
            volume_increase_action,
            volume_decrease_action,
            dump_pipeline_action,
            sender,
            receiver,
            player_receiver: None,
        };

        video_player
    }

    pub fn setup(&mut self) {
        //self.player = Some(player);
    }

    pub fn post_init(&mut self) {
        self.app.post_init(&self.player);
    }

    pub fn quit(&self) {
        self.player.write_last_known_media_position();
        self.leave_fullscreen();
        self.app.stop();
    }

    pub fn start(&mut self) {
        eprintln!("start");

        let (player_sender, player_receiver) = channel::unbounded();
        self.player_receiver = Some(player_receiver);

        let cache_file_path = if let Some(d) = ProjectDirs::from("net", "baseart", "Glide") {
            create_dir_all(d.cache_dir()).unwrap();
            Some(d.cache_dir().join("media-cache.json"))
        } else {
            None
        };

        let player_name: std::string::String = self.player.name().into();
        if let Some(context) = self.app.glib_context() {
            context.invoke(|| {
                register_player(player_name, player_sender, cache_file_path);
            });
        } else {
            register_player(player_name, player_sender, cache_file_path);
        }

        let callback = || glib::idle_add(ui_action_handle);
        let sender = self.sender.clone();
        let receiver = self.player_receiver.clone().unwrap();

        thread::spawn(move || loop {
            if let Ok(event) = receiver.try_recv() {
                // if let PlayerEvent::EndOfPlaylist = event {
                //     sender.send(UIAction::Quit).unwrap();
                //     callback();
                //     break;
                // }
                dbg!(&event);
                sender.send(UIAction::ForwardedPlayerEvent(event)).unwrap();
                callback();
            }
            thread::sleep(time::Duration::from_millis(50));
        });

        self.pause_action.connect_change_state(|pause_action, _| {
            if let Some(is_paused) = pause_action.get_state() {
                let paused = is_paused.get::<bool>().unwrap();

                with_video_player!(video_player {
                    video_player.player.toggle_pause(paused);
                });
                pause_action.set_state(&(!paused).to_variant());
            }
        });

        self.dump_pipeline_action.connect_activate(|_, _| {
            with_video_player!(video_player {
                video_player.player.dump_pipeline("glide");
            });
        });

        self.seek_forward_action.connect_change_state(|_, _| {
            with_video_player!(video_player {
                video_player.player.seek(&SeekDirection::Forward(SEEK_FORWARD_OFFSET));
            });
        });

        self.seek_backward_action.connect_change_state(|_, _| {
            with_video_player!(video_player {
                video_player.player.seek(&SeekDirection::Backward(SEEK_BACKWARD_OFFSET));
            });
        });

        self.volume_decrease_action.connect_change_state(|_, _| {
            with_video_player!(video_player {
                    video_player.player.decrease_volume();
            });
        });

        self.volume_increase_action.connect_change_state(|_, _| {
            with_video_player!(video_player {
                video_player.player.increase_volume();
            });
        });

        self.audio_mute_action.connect_change_state(|mute_action, _| {
            with_video_player!(video_player {
                if let Some(is_enabled) = mute_action.get_state() {
                    let enabled = is_enabled.get::<bool>().unwrap();
                    video_player.player.toggle_mute(!enabled);
                    mute_action.set_state(&(!enabled).to_variant());
                }
            });
        });

        self.fullscreen_action.connect_change_state(|fullscreen_action, _| {
            if let Some(is_fullscreen) = fullscreen_action.get_state() {
                with_video_player!(video_player {
                    let fullscreen = is_fullscreen.get::<bool>().unwrap();
                    if !fullscreen {
                        video_player.app.enter_fullscreen();
                    } else {
                        video_player.app.leave_fullscreen();
                    }
                    let new_state = !fullscreen;
                    fullscreen_action.set_state(&new_state.to_variant());
                });
            }
        });

        self.restore_action.connect_change_state(|_, _| {
            with_video_player!(video_player {
                video_player.leave_fullscreen();
            });
        });

        self.subtitle_action.connect_change_state(|_, value| {
            with_video_player!(video_player {
                video_player.update_subtitle_track(value);
            });
        });

        self.audio_visualization_action.connect_change_state(|action, value| {
            if let Some(val) = value.clone() {
                if let Some(name) = val.get::<std::string::String>() {
                    with_video_player!(video_player {
                        if name == "none" {
                            video_player.player.set_audio_visualization(None);
                        } else {
                            video_player.player.set_audio_visualization(Some(AudioVisualization(name)));
                        }
                        action.set_state(&val);
                    });
                }
            }
        });

        self.audio_track_action.connect_change_state(|action, value| {
            if let Some(val) = value.clone() {
                if let Some(idx) = val.get::<std::string::String>() {
                    let (_prefix, idx) = idx.split_at(6);
                    let idx = idx.parse::<i32>().unwrap();

                    with_video_player!(video_player {
                        video_player.player.set_audio_track_index(idx);
                        action.set_state(&val);
                    });
                }
            }
        });

        self.video_track_action.connect_change_state(|action, value| {
            if let Some(val) = value.clone() {
                if let Some(idx) = val.get::<std::string::String>() {
                    let (_prefix, idx) = idx.split_at(6);
                    let idx = idx.parse::<i32>().unwrap();

                    with_video_player!(video_player {
                        video_player.player.set_video_track_index(idx);
                        action.set_state(&val);
                    });
                }
            }
        });

        self.open_media_action.connect_activate(|_, _| {
            with_video_player!(video_player {
                if let Some(uri) = video_player.app.dialog_result(video_player.player.get_current_uri()) {
                    println!("loading {}", &uri);
                    video_player.player.stop();
                    video_player.player.load_uri(&uri);
                }
            });
        });

        self.open_subtitle_file_action.connect_activate(|_, _| {
            with_video_player!(video_player {
                if let Some(uri) = video_player.app.dialog_result(video_player.player.get_current_uri()) {
                    video_player.player.configure_subtitle_track(Some(SubtitleTrack::External(uri.into())));
                }
                video_player.refresh_subtitle_track_menu();
            });
        });

        self.player.set_app(&*self.app);

        #[cfg(feature = "self-updater")]
        match self.check_update() {
            Ok(o) => {
                match o {
                    self_update::Status::UpToDate(_version) => {}
                    _ => println!("Update succeeded: {}", o),
                };
            }
            Err(e) => eprintln!("Update failed: {}", e),
        };

        #[cfg(not(target_os = "macos"))]
        self.app.start();
    }

    pub fn dispatch_event(&self, event: &PlayerEvent) {
        match event {
            PlayerEvent::MediaInfoUpdated => {
                self.media_info_updated();
            }
            PlayerEvent::PositionUpdated => {
                self.position_updated();
            }
            PlayerEvent::VideoDimensionsChanged(width, height) => {
                self.video_dimensions_changed(*width, *height);
            }
            PlayerEvent::StateChanged(ref s) => {
                self.playback_state_changed(s);
            }
            PlayerEvent::VolumeChanged(volume) => {
                self.volume_changed(*volume);
            }
            PlayerEvent::Error(msg) => {
                self.player_error(msg.to_string());
            }
            _ => {}
        };
    }

    pub fn load_playlist(&self, playlist: Vec<std::string::String>) {
        self.player.load_playlist(playlist);
    }

    pub fn player_error(&self, msg: std::string::String) {
        // FIXME: display some GTK error dialog...
        eprintln!("Internal player error: {}", msg);
        self.quit();
    }

    pub fn volume_changed(&self, volume: f64) {
        self.app.volume_changed(volume);
    }

    pub fn playback_state_changed(&self, playback_state: &PlaybackState) {
        self.app.playback_state_changed(playback_state);
    }

    pub fn video_dimensions_changed(&self, width: i32, height: i32) {
        self.app.resize_window(width, height);
    }

    pub fn media_info_updated(&self) {
        if let Some(info) = self.player.get_media_info() {
            if let Some(uri) = self.player.get_current_uri() {
                if let Some(title) = info.get_title() {
                    self.app.set_window_title(&*title);
                } else if let Ok((filename, _)) = glib::filename_from_uri(&uri) {
                    self.app.set_window_title(&filename.as_os_str().to_string_lossy());
                } else {
                    self.app.set_window_title(&uri);
                }

                if let Some(duration) = info.get_duration().seconds() {
                    self.app.set_position_range_end(duration as f64);
                }

                // Look for a matching subtitle file in same directory.
                if let Ok((mut path, _)) = glib::filename_from_uri(&uri) {
                    path.set_extension("srt");
                    let subfile = path.as_path();
                    if subfile.is_file() {
                        if let Ok(suburi) = glib::filename_to_uri(subfile, None) {
                            self.player
                                .configure_subtitle_track(Some(SubtitleTrack::External(suburi)));
                        }
                    }
                }
            }
            self.refresh_subtitle_track_menu();
            self.fill_audio_track_menu(&info);
            self.fill_video_track_menu(&info);

            if info.get_number_of_video_streams() == 0 {
                self.fill_audio_visualization_menu();
                // TODO: Might be nice to enable the first audio
                // visualization by default but it doesn't work
                // yet. See also
                // https://bugzilla.gnome.org/show_bug.cgi?id=796552
                self.audio_visualization_action.set_enabled(true);
            } else {
                self.player.refresh_video_renderer();
                self.app.clear_audio_visualization_menu();
                self.audio_visualization_action.set_enabled(false);
            }
        }
    }

    pub fn position_updated(&self) {
        if let Some(position) = self.player.get_position().seconds() {
            self.app.set_position_range_value(position);
        }
    }

    pub fn update_subtitle_track(&self, value: Option<&glib::Variant>) {
        if let Some(val) = value {
            if let Some(val) = val.get::<std::string::String>() {
                let track = if val == "none" {
                    None
                } else {
                    let (prefix, asset) = val.split_at(4);
                    if prefix == "ext-" {
                        Some(SubtitleTrack::External(asset.into()))
                    } else {
                        let idx = asset.parse::<i32>().unwrap();
                        Some(SubtitleTrack::Inband(idx))
                    }
                };
                self.player.configure_subtitle_track(track);
            }
            self.subtitle_action.set_state(&val);
        }
    }

    pub fn refresh_subtitle_track_menu(&self) {
        let section = gio::Menu::new();

        if let Some(info) = self.player.get_media_info() {
            let mut i = 0;
            let item = gio::MenuItem::new(Some("Disable"), Some("none"));
            item.set_detailed_action("app.subtitle::none");
            section.append_item(&item);

            for sub_stream in info.get_subtitle_streams() {
                let default_title = format!("Track {}", i + 1);
                let title = match sub_stream.get_tags() {
                    Some(tags) => match tags.get::<gst::tags::Title>() {
                        Some(val) => std::string::String::from(val.get().unwrap()),
                        None => default_title,
                    },
                    None => default_title,
                };
                let lang = sub_stream.get_language().map(|l| {
                    if l == title {
                        "".to_string()
                    } else {
                        format!(" - [{}]", l)
                    }
                });

                let action_label = format!("{}{}", title, lang.unwrap_or_else(|| "".to_string()));
                let action_id = format!("app.subtitle::sub-{}", i);
                let item = gio::MenuItem::new(Some(&action_label), Some(&action_id));
                item.set_detailed_action(&*action_id);
                section.append_item(&item);
                i += 1;
            }
        }

        let mut selected_action: Option<std::string::String> = None;
        if let Some(uri) = self.player.get_subtitle_uri() {
            if let Ok((path, _)) = glib::filename_from_uri(&uri) {
                let subfile = path.as_path();
                if let Some(filename) = subfile.file_name() {
                    if let Some(f) = filename.to_str() {
                        let v = format!("ext-{}", uri);
                        let action_id = format!("app.subtitle::{}", v);
                        let item = gio::MenuItem::new(Some(f), Some(&action_id));
                        item.set_detailed_action(&*action_id);
                        section.append_item(&item);
                        selected_action = Some(v);
                    }
                }
            }
        }

        self.app.update_subtitle_track_menu(&section);

        let v = match selected_action {
            Some(a) => a.to_variant(),
            None => ("none").to_variant(),
        };
        self.subtitle_action.change_state(&v);
    }

    pub fn fill_audio_visualization_menu(&self) {
        if !self.app.mutable_audio_visualization_menu() {
            return;
        }
        let section = gio::Menu::new();

        let item = gio::MenuItem::new(Some("Disable"), Some("none"));
        item.set_detailed_action("app.audio-visualization::none");
        section.append_item(&item);

        for vis in gst_player::Player::visualizations_get() {
            let action_id = format!("app.audio-visualization::{}", vis.name());
            let item = gio::MenuItem::new(Some(vis.description()), Some(&action_id));
            item.set_detailed_action(&*action_id);
            section.append_item(&item);
        }

        self.app.update_audio_visualization_menu(&section);
    }

    pub fn fill_audio_track_menu(&self, info: &gst_player::PlayerMediaInfo) {
        let section = gio::Menu::new();

        let item = gio::MenuItem::new(Some("Disable"), Some("subtitle"));
        item.set_detailed_action("app.audio-track::audio--1");
        section.append_item(&item);

        for (i, audio_stream) in info.get_audio_streams().iter().enumerate() {
            let mut label = format!("{} channels", audio_stream.get_channels());
            if let Some(l) = audio_stream.get_language() {
                label = format!("{} - [{}]", label, l);
            }
            let action_id = format!("app.audio-track::audio-{}", i);
            let item = gio::MenuItem::new(Some(&label), Some(&action_id));
            item.set_detailed_action(&*action_id);
            section.append_item(&item);
        }
        self.app.update_audio_track_menu(&section);
    }

    pub fn fill_video_track_menu(&self, info: &gst_player::PlayerMediaInfo) {
        let section = gio::Menu::new();

        let item = gio::MenuItem::new(Some("Disable"), Some("subtitle"));
        item.set_detailed_action("app.video-track::video--1");
        section.append_item(&item);

        for (i, video_stream) in info.get_video_streams().iter().enumerate() {
            let action_id = format!("app.video-track::video-{}", i);
            let description = format!("{}x{}", video_stream.get_width(), video_stream.get_height());
            let item = gio::MenuItem::new(Some(&description), Some(&action_id));
            item.set_detailed_action(&*action_id);
            section.append_item(&item);
        }
        self.app.update_video_track_menu(&section);
    }

    #[cfg(feature = "self-updater")]
    pub fn check_update(&self) -> Result<self_update::Status, self_update::errors::Error> {
        let target = self_update::get_target()?;
        if let Ok(mut b) = self_update::backends::github::Update::configure() {
            return b
                .repo_owner("philn")
                .repo_name("glide")
                .bin_name("glide")
                .target(&target)
                .current_version(cargo_crate_version!())
                .build()?
                .update();
        }

        Ok(self_update::Status::UpToDate(std::string::String::from("OK")))
    }

    pub fn leave_fullscreen(&self) {
        let fullscreen_action = &self.fullscreen_action;
        if let Some(is_fullscreen) = fullscreen_action.get_state() {
            let fullscreen = is_fullscreen.get::<bool>().unwrap();

            if fullscreen {
                self.app.leave_fullscreen();
                fullscreen_action.set_state(&false.to_variant());
            }
        }
    }
}