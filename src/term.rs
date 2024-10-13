use std::thread::sleep;
use std::time::Duration;
use std::sync::mpsc::Receiver;

pub type Signal = u8;

pub fn draw_percentage(rx: Receiver::<Signal>, msgs: String) {
    let mut percentage = None;
    loop {
        let Ok(msg) = rx.try_recv() else  {
            print!("\x1B[2J\x1B[H");
            println!("{msgs}");
            if let Some(perc) = percentage {
                println!("{perc}%..")
            }
            sleep(Duration::from_secs_f32(0.5));
            continue
        };

        match msg {
            0 => return,
            perc @ _ => percentage = Some(perc)
        }
    }
}
