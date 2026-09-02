/* The T114 bootloader's S140 v6 layout reserves flash below 0x26000 and the
   first 0x6000 bytes of RAM. The Adafruit nRF52 bootloader, its settings page,
   and the MBR parameter page sit above 0xEC000, so the application region is
   0x26000..0xEC000. Writing outside that window destroys DFU.

   The top six pages of the application region are three independent A/B pairs:
   the durable control journal at 0xE6000..0xE8000, the durable announce
   reservation at 0xE8000..0xEA000, then the identity store at
   0xEA000..0xEC000. FLASH is shortened by exactly those six pages so the
   linker can never place code into any pair; build.rs asserts every boundary,
   which fails the build if one moves without the others. */
MEMORY
{
  FLASH : ORIGIN = 0x00026000, LENGTH = 0x000C0000
  CONTROL : ORIGIN = 0x000E6000, LENGTH = 0x00002000
  RESERVATION : ORIGIN = 0x000E8000, LENGTH = 0x00002000
  STORE : ORIGIN = 0x000EA000, LENGTH = 0x00002000
  RAM   : ORIGIN = 0x20006000, LENGTH = 232K
}
