# Rutas amigables y SEO

Dos caminos, según cuánto necesites. El primero vive en el lenguaje; el
segundo es infraestructura que no requiere tocar `linkc` para nada.

## 1. `@route`, para el caso común

Un rpc que devuelve HTML puede declarar una URL limpia y rastreable por
`GET`, además de (nunca en vez de) su dirección normal `/Servicio/rpc`:

<!-- linkc:check -->
```rust
type Post = { id: Int, slug: String, title: String, body: String }
type NewPost = { slug: String, title: String, body: String }

db { posts: Post[], }

service Blog {
  @content_type("text/html; charset=utf-8")
  @route("/blog/:slug")
  rpc page(slug: String) -> String {
    "<!doctype html><h1>" + slug + "</h1>"
  }

  // Más de un parámetro, en cualquier posición (GRAMMAR.md §3.42) -- se
  // bindean por NOMBRE, no por el orden en que aparecen en la ruta.
  @content_type("text/html; charset=utf-8")
  @route("/blog/:categoria/:slug")
  rpc pageInCategory(slug: String, categoria: String) -> String {
    "<!doctype html><h1>" + categoria + "/" + slug + "</h1>"
  }

  rpc create(slug: String, title: String, body: String) -> Post {
    db.posts.insert(NewPost { slug: slug, title: title, body: body })
  }
}

test "la pagina se arma como String" {
  assert(Blog.page("hola-mundo").contains("hola-mundo"));
  assert(Blog.pageInCategory("hola-mundo", "rust").contains("rust/hola-mundo"));
}
```

`GET /blog/hola-mundo` invoca `Blog.page("hola-mundo")` sin body, como manda
cualquier crawler; `GET /blog/rust/hola-mundo` invoca
`Blog.pageInCategory("hola-mundo", "rust")` igual. Referencia completa,
reglas de forma y límites:
[GRAMMAR.md §3.37](../GRAMMAR.md#337-routeblogslug-urls-amigables-para-seo--resuelto-alcance-acotado)
y [§3.42](../GRAMMAR.md#342-route-con-múltiples-parámetros--resuelto-alcance-acotado).

**Cuándo alcanza:** una URL humana por página (blog, ficha de producto,
`sitemap.xml`), con cualquier cantidad de parámetros tomados del path, en
cualquier posición (`/blog/:categoria/:slug`, GRAMMAR.md §3.42). Es el caso
que cubre la enorme mayoría de contenido pensado para SEO.

**Cuándo NO alcanza (a propósito):**

- Una página de error 404 propia -- los errores de c-script siempre son JSON.
- Servir archivos estáticos de verdad (imágenes, CSS) -- eso nunca fue el
  trabajo de `linkc serve`.
- Cachear, comprimir, o servir bajo un dominio/subpath distinto del puerto
  del servidor.

Para cualquiera de esos casos, el camino es el de abajo.

## 2. Proxy adelante, para todo lo demás

Un reverse proxy (nginx, Caddy, o el CDN que ya estés usando) resuelve lo
que `@route` no cubre, sin agregar nada a `linkc`: reescribe una URL
cualquiera hacia el `/Servicio/rpc` real, sirve estáticos él mismo, y decide
qué mostrar en un 404 o 500. `c-script` no necesita saber que el proxy
existe -- sigue viendo requests normales a `/Servicio/rpc` (o a una
`@route`, si el rpc declaró una).

### Ejemplo: nginx

Sirve estáticos directo desde disco y muestra una página 404 propia en vez
del JSON de error de c-script -- lo que `@route` sigue sin cubrir, aunque ya
soporte cualquier cantidad de parámetros en cualquier posición
(`/blog/:categoria/:slug` funciona tal cual dentro del `.link`, sin
necesitar el proxy para eso, GRAMMAR.md §3.42).

```nginx
server {
    listen 80;
    server_name miapp.com;

    # Estáticos, servidos por nginx -- linkc nunca los ve.
    location /static/ {
        root /var/www/miapp;
        expires 30d;
    }

    # Cualquier otra cosa: al servidor c-script tal cual (incluida
    # /blog/:categoria/:slug, que ya resuelve `@route` sin intervención
    # del proxy).
    location / {
        proxy_pass http://127.0.0.1:8787;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;

        # Página 404 propia en vez del {"error": "..."} de c-script.
        proxy_intercept_errors on;
        error_page 404 /404.html;
    }

    location = /404.html {
        root /var/www/miapp;
        internal;
    }
}
```

### Ejemplo: Caddy

Mismo resultado, Caddyfile mucho más corto (Caddy resuelve TLS solo, de
paso):

```caddyfile
miapp.com {
    handle /static/* {
        root * /var/www/miapp
        file_server
    }

    handle_errors {
        @404 expression {http.error.status_code} == 404
        rewrite @404 /404.html
        root * /var/www/miapp
        file_server
    }

    reverse_proxy 127.0.0.1:8787
}
```

### El límite real de este camino

El proxy resuelve estáticos y una 404 propia, pero la LÓGICA sigue viviendo
en c-script -- el rpc que sirve una página sigue siendo un rpc normal,
declarado con `@route` (§3.42 ya cubre cualquier cantidad de parámetros, en
cualquier posición, así que el proxy no necesita reescribir nada para eso).
Lo que el proxy sigue resolviendo, y `@route` no: estáticos de verdad
(imágenes, CSS, JS) servidos directo desde disco, sin pasar por `linkc
serve`, y una página de error propia en vez del JSON de siempre.
