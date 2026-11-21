#[cfg(all(target_arch = "wasm32", feature = "gui"))]
fn main() -> eframe::Result {
    use ttd_scheduler::gui::{self, platform};
    use web_sys::wasm_bindgen::JsCast;

    _ = eframe::WebLogger::init(log::LevelFilter::Info);

    let app = gui::App::new(platform::Web::default());

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().expect("No window").document().expect("No document");

        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("Failed to find the_canvas_id")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("the_canvas_id was not a HtmlCanvasElement");

        let web_options = eframe::WebOptions::default();

        let start_result = eframe::WebRunner::new()
            .start(canvas, web_options, Box::new(|_cc| Ok(Box::new(app))))
            .await;

        // Remove the loading text and spinner:
        if let Some(loading_text) = document.get_element_by_id("loading_text") {
            match start_result {
                Ok(()) => {
                    loading_text.remove();
                }
                Err(err) => {
                    loading_text.set_inner_html(
                        "<p> The app has crashed. See the developer console for details. </p>",
                    );
                    panic!("Failed to start eframe: {err:?}");
                }
            }
        }
    });

    Ok(())
}

#[cfg(not(all(target_arch = "wasm32", feature = "gui")))]
fn main() { unimplemented!() }
