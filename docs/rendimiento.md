# Fluidez y configuración recomendada de TerraCAD-GIS

## Qué se mejoró

- **Paletas con selecciones grandes.** Lo que las paletas Propiedades y Capas calculan de la selección (tipos de objeto y capas) se calcula una sola vez por selección y revisión del dibujo, no en cada cuadro. Con 20 000 objetos seleccionados y 400 capas el cuadro pasó de ~83 ms a ~15 ms.
- **Lista de capas.** Solo se dibujan las filas visibles; con cientos de capas el costo de la paleta bajó de ~7,5 ms a ~2 ms por cuadro.

## Configuración recomendada

1. Usar siempre el instalador (compilación *release*); una compilación de depuración es varias veces más lenta.
2. Windows: tarjeta gráfica dedicada para `terracad-gis.exe` (Configuración → Sistema → Pantalla → Gráficos).
3. Linux/BSD: si se ven cortes o desgarros, arrancar con `CADCRAFT_VSYNC=1`; si se siente retraso al dibujar, con `CADCRAFT_VSYNC=0`.
4. Con dibujos muy grandes: cerrar la paleta de propiedades cuando no se use (botón «Colapsar paletas») y mantener la lista de capas oculta o filtrada con el cuadro de búsqueda.
5. Mantener las barras de herramientas que no se usan en la paleta Tool Sets en lugar de acopladas arriba; cada barra visible se redibuja en cada cuadro.
6. Autoguardado: un intervalo de 10 min o más en dibujos pesados.
