const QUERY = document.getElementById("query");
window.onload = () => {
    QUERY.value = "";
};
const DEBOUNCE_TIMEOUT = 350;
const PATH_PREVIEW = document.createElement("div");
PATH_PREVIEW.classList.add("path-preview");
document.body.appendChild(PATH_PREVIEW);

async function search(prompt) {
    const results = document.getElementById("results");
    results.innerHTML = "";
    if (prompt.trim() === "") {
        return;
    }
    const response = await fetch("/api/search", {
        method: 'POST',
        headers: { 'Content-Type': 'text/plain' },
        body: prompt,
    });
    const json = await response.json();
    if (json.length === 0) {
        results.innerHTML = "[no matches]";
        return;
    }
    for (const [full_path, path] of json) {
        let item = document.createElement("span");
        item.textContent = path;

        item.addEventListener("mouseenter", () => {
            PATH_PREVIEW.textContent = full_path;
            PATH_PREVIEW.style.display = "block";
            PATH_PREVIEW.style.width = "auto";
            PATH_PREVIEW.style.height = "auto";
        });

        item.addEventListener("mouseleave", () => {
            PATH_PREVIEW.style.display = "none";
        });

        item.addEventListener("mousemove", (e) => {
            PATH_PREVIEW.style.left = `${e.pageX + 10}px`;
            PATH_PREVIEW.style.top = `${e.pageY - 30}px`;
        });

        item.addEventListener("click", () => {
            navigator.clipboard.writeText(full_path).then(() => {
                document.body.style.cursor = "copy";
                setTimeout(() => {
                    document.body.style.cursor = "default";
                }, 150);
            }).catch(err => {
                console.error('could not to copy: ', err);
            });
        });

        results.appendChild(item);
        results.appendChild(document.createElement("br"));
    }
}

let curr_prompt = Promise.resolve();
let curr_debounce = 0;

QUERY.addEventListener("input", (e) => {
    clearTimeout(curr_debounce);
    curr_debounce = setTimeout(() => {
        curr_prompt = curr_prompt.then(() => search(QUERY.value));
    }, DEBOUNCE_TIMEOUT);
});
