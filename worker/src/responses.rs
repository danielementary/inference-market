use common::client::types::Model;

/// Filler text; the prompt is ignored (no real inference).
const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod \
    tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud \
    exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in \
    reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint \
    occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";

fn model_name(model: Model) -> &'static str {
    match model {
        Model::Qwen38 => "Qwen38:",
        Model::Llama4 => "Llama4:",
    }
}

/// The canned response (model name, then lorem ipsum) split into tokens, with the same rule as
/// `common::prompt::count_tokens`.
pub fn tokens(model: Model) -> Vec<&'static str> {
    std::iter::once(model_name(model))
        .chain(LOREM.split_whitespace())
        .collect()
}
