// Kopier-Prompt fürs Agent-Fenster der Browser-Version. Ein Agent mit
// Browser-Werkzeug (z. B. Claude Desktop mit der Erweiterung „Claude in
// Chrome“) bedient Klangwelt über den MCP-Server in der Seite
// (`window.__klangweltWeb.mcp`, siehe docs/WEB.md) – ohne Netzwerk, ohne Port.
// Gegenstück zu `agentPrompt()` des Desktops (HTTP-Endpunkt per curl).
//
// Ein eigenes Modul, damit der Desktop-Teil des Agent-Fensters (chat.ts)
// davon unberührt bleibt.

/** Wartezeit für `chat_poll` im Prompt: kurz genug für das Zeitlimit eines
 *  JavaScript-Werkzeugs, das wir nicht kennen (geht ein Aufruf vorher zu
 *  Ende, sagt der Prompt, was dann zu tun ist). */
export const WEB_POLL_SECONDS = 20;

/** Der Prompt mit der Adresse, unter der die Seite gerade läuft. */
export function webAgentPrompt(address: string): string {
  return `Klangwelt (eine Musik-App) läuft als Webseite unter ${address}. Bediene sie über Claude in Chrome (oder ein anderes Werkzeug, das JavaScript in einem Browser-Tab ausführt) – ohne sie als MCP-Server einzurichten. Wo die Anweisungen der App von Werkzeugen sprechen, rufst du sie so auf wie hier beschrieben.

1. Öffne die Adresse in einem Tab deiner Tab-Gruppe (ist sie schon offen, nimm diesen Tab) und warte, bis \`window.__klangweltWeb\` existiert.
2. Die Seite enthält einen MCP-Server ohne Netzwerk: \`await window.__klangweltWeb.mcp(nachricht)\` nimmt eine JSON-RPC-2.0-Nachricht (als Objekt) und liefert die Antwort. Bediene Klangwelt nur darüber (mit dem JavaScript-Werkzeug), nicht durch Klicken in der Oberfläche.
3. Zuerst initialize: JSON.stringify(await window.__klangweltWeb.mcp({jsonrpc:"2.0", id:1, method:"initialize", params:{protocolVersion:"2025-06-18", capabilities:{}, clientInfo:{name:"claude-in-chrome", version:"1"}}})). Halte dich an result.instructions aus der Antwort.
4. tools/list nennt alle Werkzeuge mit Beschreibung und Parametern. Aufruf eines Werkzeugs: {jsonrpc:"2.0", id:<fortlaufend>, method:"tools/call", params:{name:"get_project", arguments:{}}} – das Ergebnis steht in result.structuredContent (als Text auch in result.content[0].text), result.isError meldet Fehler.
5. Gib Antworten als JSON.stringify(...) zurück, bei sehr großen nur die nötigen Teile.
6. Werkzeuge, die es im Browser nicht gibt, melden das – dann nicht weiter versuchen.
7. Zuhören: Die Person gibt dir Aufträge im Chat der Seite. Kannst du einen Subagenten starten, der ebenfalls das Browser-Werkzeug hat, nimm einen allgemeinen (keinen, der eingerichtete MCP-Werkzeuge voraussetzt) und gib ihm diese ganze Anleitung mit, damit du selbst ansprechbar bleibst; sonst hörst du selbst zu. Die Schleife: chat_poll mit arguments {timeoutSeconds: ${WEB_POLL_SECONDS}} aufrufen – der Aufruf wartet, bis ein Auftrag kommt, höchstens ${WEB_POLL_SECONDS} s; die Seite bleibt dabei bedienbar. Bricht das JavaScript-Werkzeug vorher mit einem Zeitlimit ab, nimm ab dann 10. Leere Liste: sofort wieder chat_poll – das ist normal. Jeden Auftrag mit den Werkzeugen ausführen und mit chat_reply (replyTo = id des Auftrags) in ein, zwei Sätzen auf Deutsch sagen, was du geändert hast; bei längeren Arbeiten vorher ein kurzes chat_reply mit done=false. Dann wieder chat_poll. Hör nicht von selbst auf – nur wenn die Person im Chat ausdrücklich „Stopp“ schreibt, verabschiede dich per chat_reply.

Sag kurz Bescheid, sobald du verbunden bist und zuhörst.`;
}
