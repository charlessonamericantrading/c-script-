// Generado con `linkc new`. Este cliente nunca se toca a mano cuando el
// backend cambia -- ver la nota en ../../main.link.

import { createGreeterClient } from "../../gen/client.ts";

async function main() {
  const greeter = createGreeterClient("http://localhost:8787");
  const greeting = await greeter.greet("Mundo");
  console.log(`${greeting.message}, ${greeting.recipient}!`);
}

main().catch((e) => {
  console.error(e);
});
