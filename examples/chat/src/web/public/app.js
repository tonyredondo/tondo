// The page of the chat: a WebSocket to the room, and a line for each
// event the server sends.
const $ = (id) => document.getElementById(id);

function show(text, kind) {
  const line = document.createElement("li");
  line.className = kind;
  line.textContent = text;
  $("lines").append(line);
  line.scrollIntoView();
}

function enter(name, room) {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const address = `${scheme}://${location.host}/ws/${encodeURIComponent(room)}?name=${encodeURIComponent(name)}`;
  const socket = new WebSocket(address);

  socket.onmessage = (message) => {
    const event = JSON.parse(message.data);
    if (event.people.length > 0) {
      $("people").textContent = event.people.join(", ");
    }
    switch (event.kind) {
      case "welcome": show(`You are in ${event.room}.`, "note"); break;
      case "joined": show(`${event.from} came in.`, "note"); break;
      case "left": show(`${event.from} left.`, "note"); break;
      case "missed": show(`${event.text} messages went by while you were away.`, "note"); break;
      case "said": show(`${event.from}: ${event.text}`, "said"); break;
    }
  };
  socket.onclose = () => show("The connection closed.", "note");

  $("say").onsubmit = (submitted) => {
    submitted.preventDefault();
    if ($("text").value.trim() !== "") {
      socket.send($("text").value);
      $("text").value = "";
    }
  };
}

$("door").onsubmit = (submitted) => {
  submitted.preventDefault();
  $("door").hidden = true;
  $("chat").hidden = false;
  $("title").textContent = $("room").value;
  enter($("name").value, $("room").value);
  $("text").focus();
};
