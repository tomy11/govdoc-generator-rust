// Shared configuration and mutable app state.
//
// These files are loaded as classic <script> tags (not ES modules), so every
// top-level const/let here lives in the global lexical scope and is visible to
// the other ui/js/*.js files. state.js must load first so these are initialised
// before any other file's top-level code runs.

// Talks to the govdoc-api sidecar over HTTP. CORS on the API allows this from
// the Tauri webview origin.
const API = "http://127.0.0.1:8000";

let lastDoc = null; // { id?, doc_type, doc_data, title } for save/render/edit buttons
let expandedDocumentId = null;
const resultPanel = document.getElementById("result-section");
let currentGeneralDoc = null;
let currentGeneralPage = 1;
const generalBlocksByPage = new Map();
let selectedGeneralBlock = null;
