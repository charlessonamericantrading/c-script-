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

  rpc create(slug: String, title: String, body: String) -> Post {
    db.posts.insert(NewPost { slug: slug, title: title, body: body })
  }
}

test "la pagina se arma como String" {
  assert(Blog.page("hola-mundo").contains("hola-mundo"));
}
```

`GET /blog/hola-mundo` invoca `Blog.page("hola-mundo")` sin body, como manda
cualquier crawler. Referencia completa, reglas de forma y límites:
[GRAMMAR.md §3.37](../GRAMMAR.md#337-routeblogslug-urls-amigables-para-seo--resuelto-alcance-acotado).

**Cuándo alcanza:** una URL humana por página (blog, ficha de producto,
`sitemap.xml`), un solo parámetro tomado del path. Es el caso que cubre la
enorme mayoría de contenido pensado para SEO.

**Cuándo NO alcanza (v0, a propósito):**

- Más de un segmento dinámico (`/blog/:categoria/:slug`).
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

Sirve estáticos directo desde disco, reescribe `/blog/:categoria/:slug`
(el caso de dos parámetros que `@route` todavía no soporta) hacia un rpc
que los toma como querystring, y muestra una página 404 propia en vez del
JSON de error de c-script.

```nginx
server {
    listen 80;
    server_name miapp.com;

    # Estáticos, servidos por nginx -- linkc nunca los ve.
    location /static/ {
        root /var/www/miapp;
        expires 30d;
    }

    # /blog/tecnologia/mi-post -> POST /Blog/pageByCategory
    # con {"categoria":"tecnologia","slug":"mi-post"} en el body.
    location ~ ^/blog/([^/]+)/([^/]+)$ {
        proxy_pass http://127.0.0.1:8787/Blog/pageByCategory;
        proxy_method POST;
        proxy_set_header Content-Type "application/json";
        # nginx no arma JSON solo -- esto necesita el módulo
        # `njs`/`lua`, o resolverlo en un rpc con @route de un solo
        # parámetro y un segundo lookup adentro de c-script. La otra
        # opción, más simple, es exponer /blog/:categoria/:slug tal cual
        # como un @route de UN parámetro (`categoriaYSlug`) y separar los
        # dos campos adentro del propio rpc con `.split("/")`.
    }

    # Cualquier otra cosa: al servidor c-script tal cual.
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

El proxy resuelve el ruteo y los estáticos, pero la LÓGICA sigue viviendo
en c-script -- un rpc con dos parámetros (`categoria`, `slug`) todavía tiene
que existir, tomando ambos valores como argumentos normales. Lo único que
cambia es de dónde vienen esos valores: de un `@route` de un solo segmento,
o de la reescritura que hizo el proxy antes de que la request llegue a
`linkc serve`.
