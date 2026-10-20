//! Inspect's Compare tab: the file on view beside the one marked, fact by
//! fact, what differs lit; and the way into the two side by side, which
//! takes the whole window.

use super::devtools::{flow_x, section_head, Action};
use super::inspect::{Hit, Inspection};
use super::*;

/// The facts worth setting side by side, by name, in order.
fn facts(found: &Inspection) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    if let Some(picture) = found.picture.as_ref() {
        facts.extend(picture.format.clone().map(|format| ("Format", format)));
        facts.extend(picture.size.map(|(w, h)| ("Size", format!("{w}×{h}"))));
        facts.extend(picture.size.map(|(w, h)| ("Pixels", format!("{:.1} MP", f64::from(w) * f64::from(h) / 1e6))));
        facts.push(("File", meta::human_size(picture.bytes)));
        facts.push(("Profile", picture.icc.as_ref().map_or_else(|| "None".to_string(), |(name, _)| name.clone())));
        facts.push(("Gain map", if picture.gain.is_some() { "Yes" } else { "No" }.to_string()));
        return facts;
    }
    let format = &found.probe["format"];
    if let Some(name) = format["format_name"].as_str() {
        facts.push(("Container", name.split(',').next().unwrap_or(name).to_uppercase()));
    }
    if let Some(video) = found.video() {
        let codec = video["codec_name"].as_str().unwrap_or("video").to_uppercase();
        let profile = video["profile"].as_str().map(|p| format!(" · {p}")).unwrap_or_default();
        facts.push(("Video", format!("{codec}{profile}")));
        if let (Some(w), Some(h)) = (video["width"].as_u64(), video["height"].as_u64()) {
            facts.push(("Size", format!("{w}×{h}")));
        }
        let fps = video["avg_frame_rate"].as_str().and_then(|rate| {
            let (n, d) = rate.split_once('/')?;
            Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok().filter(|d| *d > 0.0)?)
        });
        facts.extend(fps.map(|fps| ("Frame rate", fps_words(fps))));
        facts.extend(video["pix_fmt"].as_str().map(|f| ("Pixels", f.to_string())));
        let hdr = found.hdr.as_ref().map(|hdr| hdr.badges().join(" · ")).filter(|b| !b.is_empty());
        facts.push(("HDR", hdr.unwrap_or_else(|| "SDR".to_string())));
    }
    facts.extend(format["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).map(|s| ("Length", clock(s))));
    facts.extend(format["size"].as_str().and_then(|s| s.parse::<u64>().ok()).map(|b| ("File", meta::human_size(b))));
    facts.extend(format["bit_rate"].as_str().and_then(|b| b.parse::<f64>().ok()).map(|b| ("Bitrate", format!("{:.1} Mb/s", b / 1e6))));
    let audio = found.streams().iter().find(|s| s["codec_type"] == "audio").map(|a| {
        let codec = a["codec_name"].as_str().unwrap_or("audio").to_uppercase();
        let layout = a["channel_layout"].as_str().map(|l| format!(" · {l}")).unwrap_or_default();
        format!("{codec}{layout}")
    });
    facts.push(("Audio", audio.unwrap_or_else(|| "None".to_string())));
    facts
}

impl App {
    /// The file the one on view is compared with: the first marked that
    /// is not it.
    fn compare_other(&self, id: usize) -> Option<usize> {
        self.list.iter().copied().find(|other| *other != id && self.marks.contains(other))
    }

    pub(super) fn compared(&mut self, id: usize, found: Result<Inspection, String>) {
        if let Some((asked, answer)) = &mut self.compare_found {
            if *asked == id {
                *answer = Some(found);
                self.redraw();
            }
        }
    }

    /// The facts of the two, side by side; then the way to see them so.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_compare_tab(&mut self, sub: &mut Canvas, text: &mut Text, px: u32, found: &Inspection, id: usize, width: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = flow_x();
        let other = self.compare_other(id);
        // The other file read once, on another thread, as this one was.
        if let Some(other) = other {
            if self.compare_found.as_ref().is_none_or(|(asked, _)| *asked != other) {
                self.compare_found = Some((other, None));
                let (path, proxy) = (self.files[other].clone(), self.proxy.clone());
                std::thread::spawn(move || {
                    let _ = proxy.send_event(Event::Compared(other, Inspection::read(&path)));
                });
            }
        }
        let mut y = section_head(sub, text, px, x, y, width, "Side by side");
        let Some(other) = other else {
            text.draw(sub, x, y, px, DIM_TEXT_COLOR, "Mark a file · x · to compare it with this one");
            return y + line + pad;
        };
        let name = |app: &App, id: usize| app.files[id].file_name().unwrap_or_default().to_string_lossy().into_owned();
        let mine = facts(found);
        let theirs = match self.compare_found.as_ref().and_then(|(_, answer)| answer.as_ref()) {
            Some(Ok(found)) => Some(facts(found)),
            Some(Err(error)) => {
                let words = clipped(text, &format!("Not readable · {error}"), width, px);
                text.draw(sub, x, y, px, DIM_TEXT_COLOR, &words);
                return y + line + pad;
            }
            None => None,
        };
        let name_w = text.width("Frame rate", px) + pad;
        let column = (width - name_w - pad) / 2;
        let (a_x, b_x) = (x + name_w, x + name_w + column + pad);
        // Each file's name over its column.
        let small = (px * 5 / 6).max(11);
        let this_name = clipped(text, &name(self, id), column, small);
        let that_name = clipped(text, &name(self, other), column, small);
        text.draw(sub, a_x, y, small, DIM_TEXT_COLOR, &this_name);
        text.draw(sub, b_x, y, small, DIM_TEXT_COLOR, &that_name);
        y += Text::line_height(small) + pad / 3;
        let mut names: Vec<&str> = mine.iter().map(|(name, _)| *name).collect();
        for (name, _) in theirs.iter().flatten() {
            if !names.contains(name) {
                names.push(name);
            }
        }
        let row_h = line + pad / 4;
        let mut differ = 0;
        for fact in names {
            let a = mine.iter().find(|(name, _)| *name == fact).map_or("—".to_string(), |(_, value)| value.clone());
            let b = match &theirs {
                Some(theirs) => theirs.iter().find(|(name, _)| *name == fact).map_or("—".to_string(), |(_, value)| value.clone()),
                None => "…".to_string(),
            };
            let differs = theirs.is_some() && a != b;
            differ += usize::from(differs);
            if differs {
                // A mark against the edge, and the two in full white.
                sub.fill(x - pad / 2, y + pad / 6, 2, row_h - pad / 3, accent_soft());
            }
            text.draw(sub, x, y, px, DIM_TEXT_COLOR, fact);
            let color = if differs { TEXT_COLOR } else { DIM_TEXT_COLOR };
            let a = clipped(text, &a, column, px);
            let b = clipped(text, &b, column, px);
            text.draw(sub, a_x, y, px, color, &a);
            text.draw(sub, b_x, y, px, if differs { accent_soft() } else { color }, &b);
            y += row_h;
        }
        if theirs.is_some() {
            let words = match differ {
                0 => "The same in all of these".to_string(),
                1 => "1 differs".to_string(),
                n => format!("{n} differ"),
            };
            text.draw(sub, x, y + pad / 4, small, DIM_TEXT_COLOR, &words);
            y += Text::line_height(small) + pad / 2;
        }
        y += pad;
        // Side by side in the window is for pictures.
        if files::is_video(&self.files[id]) || files::is_video(&self.files[other]) {
            return y;
        }
        let actions = vec![("Open".to_string(), vec![Action { words: "The two side by side, full window".to_string(), hit: Hit::Run(Command::Compare) }])];
        self.draw_actions(sub, text, px, &actions, width, y, offset, mouse, drawn)
    }
}
