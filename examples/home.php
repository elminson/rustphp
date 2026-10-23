<!DOCTYPE html>
<html lang="es">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>rustphp 1.0</title>
<style>
:root {
	--bg: #0f1117;
	--panel: #171a23;
	--ink: #e7e9ee;
	--muted: #9aa3b2;
	--line: #262b38;
	--accent: #e9643a;   /* PHP/FI orange-ish */
	--accent-2: #d4895a;
	--rust: #ce422b;
	--radius: 14px;
}

* { box-sizing: border-box; }

body {
	margin: 0;
	font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
	line-height: 1.6;
	color: var(--ink);
	background: var(--bg);
}

.wrap { max-width: 860px; margin: 0 auto; padding: 0 20px; }

header.hero {
	background: linear-gradient(135deg, #1b1e2a 0%, #241a18 100%);
	border-bottom: 1px solid var(--line);
	padding: 56px 0 40px;
	text-align: center;
}
header.hero h1 {
	margin: 0 0 6px;
	font-size: 2.6rem;
	letter-spacing: -0.02em;
}
header.hero h1 .dot { color: var(--accent); }
header.hero p.tag { margin: 0; color: var(--muted); font-size: 1.05rem; }

nav {
	display: flex;
	gap: 8px;
	justify-content: center;
	flex-wrap: wrap;
	padding: 16px 0;
	border-bottom: 1px solid var(--line);
	position: sticky;
	top: 0;
	background: rgba(15,17,23,0.9);
	backdrop-filter: blur(6px);
}
nav a {
	color: var(--ink);
	text-decoration: none;
	font-size: 0.9rem;
	padding: 6px 12px;
	border: 1px solid var(--line);
	border-radius: 999px;
}
nav a:hover { border-color: var(--accent); color: var(--accent-2); }

main { padding: 36px 0 20px; }

.greet {
	background: var(--panel);
	border: 1px solid var(--line);
	border-left: 3px solid var(--accent);
	border-radius: var(--radius);
	padding: 16px 20px;
	margin: 0 0 28px;
}
.greet code {
	background: #0b0d12;
	padding: 1px 6px;
	border-radius: 6px;
	color: var(--accent-2);
}

h2 {
	font-size: 1.25rem;
	margin: 32px 0 14px;
	padding-bottom: 8px;
	border-bottom: 1px solid var(--line);
}

.cards {
	display: grid;
	grid-template-columns: repeat(auto-fit, minmax(230px, 1fr));
	gap: 16px;
}
.card {
	background: var(--panel);
	border: 1px solid var(--line);
	border-radius: var(--radius);
	padding: 18px 20px;
	transition: transform 0.15s ease, border-color 0.15s ease;
}
.card:hover { transform: translateY(-3px); border-color: var(--accent); }
.card h3 { margin: 0 0 6px; font-size: 1.05rem; }
.card p { margin: 0; color: var(--muted); font-size: 0.92rem; }

table.squares {
	width: 100%;
	border-collapse: collapse;
	background: var(--panel);
	border: 1px solid var(--line);
	border-radius: var(--radius);
	overflow: hidden;
}
table.squares th, table.squares td {
	text-align: left;
	padding: 10px 16px;
	border-bottom: 1px solid var(--line);
	font-variant-numeric: tabular-nums;
}
table.squares th { color: var(--muted); font-weight: 600; font-size: 0.85rem; text-transform: uppercase; letter-spacing: 0.04em; }
table.squares tr:last-child td { border-bottom: 0; }

.meta {
	display: flex;
	gap: 10px;
	flex-wrap: wrap;
	margin-top: 10px;
}
.pill {
	font-size: 0.8rem;
	color: var(--muted);
	background: #0b0d12;
	border: 1px solid var(--line);
	border-radius: 999px;
	padding: 4px 12px;
}

footer {
	margin-top: 44px;
	padding: 24px 0 40px;
	border-top: 1px solid var(--line);
	color: var(--muted);
	font-size: 0.85rem;
	text-align: center;
}
footer .rust { color: var(--rust); font-weight: 600; }
</style>
</head>
<body>

<header class="hero">
	<div class="wrap">
		<h1>rustphp<span class="dot">.</span></h1>
		<p class="tag">PHP/FI 1.99s, reimplementado en Rust &mdash; sirviendo esta misma página</p>
	</div>
</header>

<nav>
	<div class="wrap" style="display:flex; gap:8px; justify-content:center; flex-wrap:wrap;">
		<a href="?name=Elminson">Saludo</a>
		<a href="#features">Qué trae</a>
		<a href="#demo">Demo</a>
		<a href="https://github.com/elminson/rustphp">GitHub</a>
	</div>
</nav>

<main class="wrap">

<? if($name); >
	<div class="greet">Hola, <strong><? echo $name; ></strong>. Esta página la renderizó el intérprete PHP/FI escrito en Rust.</div>
<? else; >
	<div class="greet">Agrega <code>?name=TuNombre</code> a la URL para un saludo personalizado.</div>
<? endif; >

<p>
	Método del request: <strong><? echo $REQUEST_METHOD; ></strong>.
</p>
<div class="meta">
	<span class="pill">Cero dependencias</span>
	<span class="pill">Solo std de Rust</span>
	<span class="pill">Servidor propio</span>
	<span class="pill">42 tests</span>
</div>

<h2 id="features">Qué trae</h2>
<div class="cards">
	<div class="card">
		<h3>Lexer y parser</h3>
		<p>Tokeniza los tags <code>&lt;? &gt;</code> y arma el AST con un parser recursivo.</p>
	</div>
	<div class="card">
		<h3>Intérprete</h3>
		<p>Recorre el árbol y evalúa, con los tres tipos escalares del PHP original.</p>
	</div>
	<div class="card">
		<h3>~40 builtins</h3>
		<p>strlen, sprintf, substr, date, abs, max y compañía, con su semántica vieja.</p>
	</div>
	<div class="card">
		<h3>Servidor HTTP</h3>
		<p>Un <code>--serve</code> hecho a mano que corre archivos .php sin dependencias.</p>
	</div>
</div>

<h2 id="demo">Demo: generado con un loop</h2>
<p>Esta tabla la arma un <code>while</code> dentro del template:</p>
<table class="squares">
	<tr><th>n</th><th>n al cuadrado</th></tr>
<? $i = 1;
   while($i <= 8); >
	<tr><td><? echo $i; ></td><td><? echo $i * $i; ></td></tr>
<? $i++;
   endwhile; >
</table>

</main>

<footer class="wrap">
	Servido por rustphp 1.0 &middot; escrito en <span class="rust">Rust</span> &middot; &copy; <? echo date("Y"); >
</footer>

</body>
</html>
