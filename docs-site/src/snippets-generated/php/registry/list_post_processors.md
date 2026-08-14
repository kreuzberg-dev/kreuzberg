---
id: fixture_php_list_post_processors
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

List post-processors

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
$result = Xberg::listPostProcessors();
var_dump($result);

```
