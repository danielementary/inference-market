// Generates the Rust client for the `inference_escrow` program from its Anchor IDL.
// Usage: npm run generate (from clients/codama) or scripts/generate-client.sh (from repo root).
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  bottomUpTransformerVisitor,
  constantPdaSeedNodeFromString,
  createFromRoot,
  isNode,
  pdaLinkNode,
  updateAccountsVisitor,
} from 'codama';
import { rootNodeFromAnchor } from '@codama/nodes-from-anchor';
import { renderVisitor } from '@codama/renderers-rust';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const idlPath = resolve(repoRoot, 'onchain/target/idl/inference_escrow.json');
const crateDir = resolve(repoRoot, 'clients/rust');

// Minimal base58 decoder (nodes-from-anchor stores constant byte seeds as base58).
const B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function base58ToBytes(str) {
  let n = 0n;
  for (const c of str) n = n * 58n + BigInt(B58.indexOf(c));
  const bytes = [];
  while (n > 0n) {
    bytes.unshift(Number(n & 0xffn));
    n >>= 8n;
  }
  for (const c of str) {
    if (c !== '1') break;
    bytes.unshift(0);
  }
  return Uint8Array.from(bytes);
}

const idl = JSON.parse(readFileSync(idlPath, 'utf8'));
const codama = createFromRoot(rootNodeFromAnchor(idl));

// Anchor IDLs encode constant seeds such as b"config" as raw bytes, which the Rust renderer
// emits as `[..].as_bytes()` (does not compile). Turn printable-ASCII byte seeds into string seeds.
codama.update(
  bottomUpTransformerVisitor([
    {
      select: '[constantPdaSeedNode]',
      transform: (node) => {
        if (!isNode(node.value, 'bytesValueNode') || node.value.encoding !== 'base58') return node;
        const bytes = base58ToBytes(node.value.data);
        if (!bytes.every((b) => b >= 0x20 && b < 0x7f)) return node;
        return constantPdaSeedNodeFromString('utf8', new TextDecoder().decode(bytes));
      },
    },
  ]),
);

// nodes-from-anchor extracts the PDAs from instruction seeds but does not link them to the
// account types; linking them makes the Rust renderer emit `find_pda` / `create_pda` helpers.
codama.update(
  updateAccountsVisitor({
    config: { pda: pdaLinkNode('config') },
    request: { pda: pdaLinkNode('request') },
    worker: { pda: pdaLinkNode('worker') },
  }),
);

codama.accept(
  renderVisitor(crateDir, {
    generatedFolder: 'src/generated',
    deleteFolderBeforeRendering: true,
    formatCode: true,
    // Skip the optional anchor-lang trait impls; off-chain consumers only need borsh.
    anchorTraits: false,
  }),
);

console.log(`Rendered Rust client from ${idlPath} into ${crateDir}/src/generated`);
