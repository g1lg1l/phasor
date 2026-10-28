//! Lessons (M10): guided first steps. A card in the corner says what to do next; each step
//! checks the project (or what's on screen) and moves on by itself when it's done, with a
//! burst of sparks. Finished lessons are remembered.

use eframe::egui::{self, Align2, Frame, Id, RichText, pos2, vec2};
use ravune_core::{Instrument, STEP};

use crate::{App, Tab, icons, kit};

pub struct Step {
    pub text: &'static str,
    pub hint: &'static str,
    pub done: fn(&App) -> bool,
}

pub struct Lesson {
    pub title: &'static str,
    pub about: &'static str,
    pub steps: &'static [Step],
}

fn channel<'a>(app: &'a App, name: &str) -> Option<(usize, &'a ravune_core::Channel)> {
    app.session.project.channels.iter().enumerate().find(|(_, c)| c.name.eq_ignore_ascii_case(name))
}

/// Does `name`'s channel have steps on all of `steps` in the current pattern?
fn has_steps(app: &App, name: &str, steps: &[usize]) -> bool {
    channel(app, name).is_some_and(|(_, c)| steps.iter().all(|s| app.session.current_pattern().step(c.slot, *s) > 0))
}

fn synth_notes(app: &App) -> usize {
    let pattern = app.session.current_pattern();
    app.session.project.channels.iter().filter(|c| matches!(c.instrument, Instrument::Synth { .. })).map(|c| pattern.notes(c.slot).len()).sum()
}

pub const LESSONS: [Lesson; 4] = [
    Lesson {
        title: "Your first beat",
        about: "Kick, clap and hats in the channel rack, then play it",
        steps: &[
            Step { text: "Start from a new project (Ctrl N): it comes with a kick, a clap and hats.", hint: "File → New", done: |a| ["Kick", "Clap", "Hat"].iter().all(|n| channel(a, n).is_some()) },
            Step { text: "Open the Channel Rack.", hint: "F6, or its tab at the bottom", done: |a| a.tab == Tab::Rack || a.split == Some(Tab::Rack) },
            Step { text: "A kick on every beat: click the Kick's steps 1, 5, 9 and 13.", hint: "Each square is a 16th note; drag to paint several", done: |a| has_steps(a, "Kick", &[0, 4, 8, 12]) },
            Step { text: "Press Space to hear it.", hint: "Space plays and stops", done: |a| a.session.playing },
            Step { text: "A clap on beats 2 and 4: the Clap's steps 5 and 13.", hint: "It plays while you click", done: |a| has_steps(a, "Clap", &[4, 12]) },
            Step { text: "Hats between the kicks: the Hat's steps 3, 7, 11 and 15.", hint: "The off-beats make it bounce", done: |a| has_steps(a, "Hat", &[2, 6, 10, 14]) },
            Step { text: "Make it swing: drag the swing up a little.", hint: "The swing box above the steps", done: |a| a.session.project.swing > 0.05 },
        ],
    },
    Lesson {
        title: "A bassline",
        about: "Notes for the VA-1 synth in the piano roll",
        steps: &[
            Step { text: "Select the VA-1 channel in the rack (click its name).", hint: "The keyboard and the piano roll follow the selected channel", done: |a| a.session.selected_channel().is_some_and(|c| matches!(c.instrument, Instrument::Synth { .. })) },
            Step { text: "Open the Piano Roll.", hint: "F7, or double-click the channel's name", done: |a| a.tab == Tab::PianoRoll || a.split == Some(Tab::PianoRoll) },
            Step { text: "Draw four notes, low down, between the kicks.", hint: "Click to draw, drag to move, right-click to delete", done: |a| synth_notes(a) >= 4 },
            Step { text: "Play it with the beat (Space).", hint: "Space plays the pattern", done: |a| a.session.playing && synth_notes(a) >= 4 },
            Step {
                text: "Make the notes shorter and punchier: Tools → Legato, or drag their ends.",
                hint: "Short notes leave room for the kick",
                done: |a| {
                    let pattern = a.session.current_pattern();
                    a.session.project.channels.iter().filter(|c| matches!(c.instrument, Instrument::Synth { .. })).any(|c| pattern.notes(c.slot).iter().any(|n| n.length != STEP))
                },
            },
        ],
    },
    Lesson {
        title: "From a loop to a song",
        about: "More patterns, the playlist, and a song laid out for you",
        steps: &[
            Step { text: "Make a second pattern (F4) and put something in it.", hint: "The pattern list is in the rack's bar", done: |a| a.session.project.patterns.iter().filter(|p| p.notes.values().any(|n| !n.is_empty())).count() >= 2 },
            Step { text: "Switch to SONG mode: the playlist plays instead of the pattern.", hint: "PAT / SONG in the top bar", done: |a| a.session.song_mode },
            Step { text: "Let Ravune lay it out: Create → Song from the patterns.", hint: "Intro, build, drop, break, drop 2, outro, with markers", done: |a| a.session.project.markers.len() >= 6 },
            Step { text: "Play the song from the start (Home, then Space).", hint: "Click a marker on the ruler to jump there", done: |a| a.session.playing && a.session.song_mode },
        ],
    },
    Lesson {
        title: "A mix that breathes",
        about: "The mixer, sidechain and a reverb send",
        steps: &[
            Step { text: "Open the Mixer.", hint: "F9", done: |a| a.tab == Tab::Mixer || a.split == Some(Tab::Mixer) || a.settings.floating.contains(&Tab::Mixer) },
            Step { text: "Duck the bass under the kick: right-click the bass's track → Duck to → Kick.", hint: "Or the Duck to… button over its effects", done: |a| a.session.project.mixer.iter().flat_map(|t| &t.devices).any(|d| d.sidechain.is_some()) },
            Step { text: "Add an effect to a track: + Effect in its effects panel.", hint: "Reverb, delay, saturator…", done: |a| a.session.project.mixer.iter().skip(1).any(|t| !t.devices.is_empty()) },
            Step { text: "Send a track to another one: pull a cable from its jack with Shift held.", hint: "A send keeps the track and adds a copy, e.g. to a reverb", done: |a| a.session.project.mixer.iter().any(|t| !t.sends.is_empty()) },
        ],
    },
];

impl App {
    pub fn start_lesson(&mut self, k: usize) {
        self.lesson = Some((k, 0));
    }

    /// The lesson card, bottom right: the step, its hint, progress; it moves on by itself.
    pub fn lesson_card(&mut self, ctx: &egui::Context, now: f64) {
        let Some((k, step)) = self.lesson else { return };
        let lesson = &LESSONS[k];
        // every step that's already done goes by (a lesson picked up halfway skips ahead)
        let mut step = step;
        let mut advanced = false;
        while step < lesson.steps.len() && (lesson.steps[step].done)(self) {
            step += 1;
            advanced = true;
        }
        let t = self.theme.clone();
        let finished = step >= lesson.steps.len();
        let (mut close, mut next) = (false, None);
        let screen = ctx.content_rect();
        let area = egui::Area::new(Id::unique("lesson-card")).order(egui::Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -36.0));
        let shown = area.show(ctx, |ui| {
            kit::window_frame(ctx, &t).show(ui, |ui| {
                ui.set_width(300.0_f32.min(screen.width() - 32.0));
                kit::region(ui);
                kit::bar(ui, &t, |ui| {
                    ui.label(RichText::new(format!("{}  {}", icons::GRADUATION_CAP, crate::i18n::t(lesson.title))).strong());
                    kit::right(ui, |ui| close = kit::icon_button(ui, &t, icons::X, tr!("Close the lesson"), false).clicked());
                });
                Frame::new().inner_margin(t.pad + 2.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = t.gap * 1.5;
                    // progress: a dot per step
                    let (dots, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), egui::Sense::hover());
                    for s in 0..lesson.steps.len() {
                        let c = pos2(dots.left() + 4.0 + s as f32 * 12.0, dots.center().y);
                        let color = if s < step { t.accent } else if s == step { t.fg_text } else { t.bg_control };
                        ui.painter().circle_filled(c, 3.5, color);
                    }
                    if finished {
                        ui.label(RichText::new(tr!("Done! That's the lesson.")).size(15.0).color(t.accent));
                        ui.horizontal(|ui| {
                            if k + 1 < LESSONS.len() && kit::button(ui, &t, Some(icons::ARROW_RIGHT), &tr!("Next: {}", crate::i18n::t(LESSONS[k + 1].title)), true).clicked() {
                                next = Some(k + 1);
                            }
                            if kit::button(ui, &t, None, tr!("Close"), false).clicked() {
                                close = true;
                            }
                        });
                    } else {
                        let s = &lesson.steps[step];
                        ui.label(RichText::new(tr!("Step {} of {}", step + 1, lesson.steps.len())).small().color(t.fg_muted));
                        ui.label(RichText::new(crate::i18n::t(s.text)).size(14.0).color(t.fg_text));
                        ui.label(RichText::new(format!("{}  {}", icons::LIGHTBULB, crate::i18n::t(s.hint))).small().color(t.fg_muted));
                        if kit::button(ui, &t, None, tr!("Skip this step"), false).clicked() {
                            step += 1;
                        }
                    }
                });
            });
        });
        if advanced && !self.settings.reduce_motion {
            // a burst of sparks from the card
            let at = shown.response.rect.left_top() + vec2(18.0, 46.0);
            for (dx, color) in [(0.0, t.accent), (60.0, t.select), (120.0, t.accent), (180.0, t.meter_ok)] {
                self.sparks.push(kit::Spark { pos: at + vec2(dx, 0.0), at: now, color });
            }
        }
        if finished && !self.settings.lessons_done.iter().any(|d| d == lesson.title) {
            self.settings.lessons_done.push(lesson.title.to_string());
        }
        self.lesson = match (close, next) {
            (true, _) => None,
            (_, Some(n)) => Some((n, 0)),
            _ => Some((k, step)),
        };
    }

    /// Help → Lessons.
    pub fn lessons_menu(&mut self, ui: &mut egui::Ui) {
        let t = self.theme.clone();
        for (k, lesson) in LESSONS.iter().enumerate() {
            let done = self.settings.lessons_done.iter().any(|d| d == lesson.title);
            let icon = if done { icons::CHECK } else { icons::GRADUATION_CAP };
            let r = ui.add(egui::Button::new(format!("{icon}  {}", crate::i18n::t(lesson.title))).shortcut_text(RichText::new(format!("{}", lesson.steps.len())).color(t.fg_muted)));
            if r.on_hover_text(crate::i18n::t(lesson.about)).clicked() {
                self.start_lesson(k);
                ui.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Doing what "Your first beat" asks, step by step, ticks every step off in turn.
    #[test]
    fn the_first_beat_lesson_follows_the_project() {
        let mut app = App::for_tests();
        let steps = LESSONS[0].steps;
        let done = |app: &App| steps.iter().take_while(|s| (s.done)(app)).count();
        app.tab = Tab::PianoRoll;
        assert_eq!(done(&app), 1, "a new project has a kick, a clap and hats");
        app.tab = Tab::Rack;
        assert_eq!(done(&app), 2);
        for step in [0, 4, 8, 12] {
            app.session.run("step.set", json!({ "channel": 0, "step": step, "velocity": 110 })).unwrap();
        }
        assert_eq!(done(&app), 3);
        app.session.playing = true;
        for step in [4, 12] {
            app.session.run("step.set", json!({ "channel": 2, "step": step, "velocity": 100 })).unwrap();
        }
        for step in [2, 6, 10, 14] {
            app.session.run("step.set", json!({ "channel": 3, "step": step, "velocity": 80 })).unwrap();
        }
        assert_eq!(done(&app), 6);
        app.session.run("transport.set_swing", json!({ "amount": 0.3 })).unwrap();
        assert_eq!(done(&app), steps.len(), "the whole lesson");
    }
}
