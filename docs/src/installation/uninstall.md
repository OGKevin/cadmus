# Uninstalling Cadmus

To remove Cadmus from your Kobo:

1. Connect your Kobo to your computer via USB.
2. Delete the Cadmus folder from `.adds`:

   | Build  | Folder to delete                |
   | ------ | ------------------------------- |
   | Stable | `/mnt/onboard/.adds/cadmus`     |
   | Test   | `/mnt/onboard/.adds/cadmus-tst` |

3. If you installed a package that included NickelMenu, delete the Cadmus menu
   entry too:

   | Build  | NickelMenu entry to delete         |
   | ------ | ---------------------------------- |
   | Stable | `/mnt/onboard/.adds/nm/cadmus`     |
   | Test   | `/mnt/onboard/.adds/nm/cadmus-tst` |

4. Eject the device and disconnect the USB cable.

> [!NOTE]
> If you no longer need NickelMenu at all, you can remove it separately.
