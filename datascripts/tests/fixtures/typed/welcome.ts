function welcome(event: PlayerLoginEvent): void {
  send_chat(event.player, "Welcome, " + event.player.name);
}

events.player.onLogin(welcome);
