/* The T114 bootloader's S140 v6 layout reserves flash below 0x26000 and the
   first 0x6000 bytes of RAM. The Adafruit nRF52 bootloader, its settings page,
   and the MBR parameter page sit above 0xEC000, so the application region is
   0x26000..0xEC000. Writing outside that window destroys DFU.

   The top two pages of the application region hold the radio-hand identity
   store. FLASH is shortened by exactly those two pages so the linker can never
   place code into them; build.rs asserts that FLASH ends where STORE begins,
   which fails the build if one moves without the other. */
MEMORY
{
  FLASH : ORIGIN = 0x00026000, LENGTH = 0x000C4000
  STORE : ORIGIN = 0x000EA000, LENGTH = 0x00002000
  RAM   : ORIGIN = 0x20006000, LENGTH = 232K
}
