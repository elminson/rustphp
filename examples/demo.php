<html><head><title>rustphp .php demo</title></head>
<body>
<h1>rustphp — a .php file</h1>

<?
/* PHP/FI 1.x uses <? ... > tags (not the modern <?php ... ?>). */
echo "Request method: $REQUEST_METHOD<br>\n";
echo "You are: $HTTP_USER_AGENT<br>\n";

/* $name comes from the query string, e.g. demo.php?name=Elminson */
if($name);
	echo "Hello, $name!<br>\n";
else;
	echo "Add ?name=YourName to the URL to be greeted.<br>\n";
endif;

echo "<hr>Counting with a while loop:<br>\n";
$i = 1;
while($i <= 5);
	echo "%2d squared is %3d<br>\n" $i, $i * $i;
	$i++;
endwhile;
>
</body></html>
